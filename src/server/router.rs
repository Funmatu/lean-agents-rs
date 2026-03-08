use std::convert::Infallible;

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::domain::context::ContextGraph;
use crate::domain::event::EngineEvent;
use crate::engine::Engine;

use super::state::AppState;

/// Request body for the stream endpoint.
#[derive(Debug, Deserialize)]
pub struct StreamRequest {
    pub task: String,
}

/// Build the axum router with all API routes.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/agent/stream", post(stream_handler))
        .with_state(state)
}

/// POST /v1/agent/stream
///
/// 1. Acquire semaphore permit (blocks if at capacity)
/// 2. Spawn engine in background with MPSC channel
/// 3. Return SSE stream of EngineEvents
async fn stream_handler(
    State(state): State<AppState>,
    Json(payload): Json<StreamRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if payload.task.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "task cannot be empty".into()));
    }

    // Acquire semaphore permit — blocks until a slot is available.
    // Using acquire_owned so the permit can be moved into the spawned task.
    let permit = state
        .concurrency_limiter
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "concurrency limiter closed".into(),
            )
        })?;

    let (tx, rx) = mpsc::channel::<EngineEvent>(64);

    let llm = state.llm.clone();
    let search = state.search.clone();
    let task = payload.task;

    // Spawn engine execution in background.
    // The permit is moved in — it will be dropped (released) when this task ends,
    // whether by normal completion, error, or panic.
    tokio::spawn(async move {
        let _permit = permit; // Hold permit for task lifetime (RAII release)
        let engine = Engine::new();
        let mut context = ContextGraph::new();

        let result = engine
            .run(&mut context, llm.as_ref(), search.as_ref(), &task, &tx)
            .await;

        if let Err(e) = result {
            // Best-effort: send error event before closing channel
            let _ = tx
                .send(EngineEvent::WorkflowEscalated {
                    reason: e.to_string(),
                })
                .await;
        }
        // tx drops here → ReceiverStream ends → SSE stream closes
    });

    // Convert receiver to SSE stream
    let stream = ReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
        Ok::<_, Infallible>(Event::default().data(data))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::llm::tests::MockLlmClient;
    use crate::client::search::tests::MockSearchClient;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state(llm_responses: Vec<String>) -> AppState {
        AppState::new(
            Arc::new(MockLlmClient::new(llm_responses)),
            Arc::new(MockSearchClient::new(vec![])),
            2,
        )
    }

    #[tokio::test]
    async fn empty_task_returns_400() {
        let app = build_router(test_state(vec![]));
        let req = Request::builder()
            .method("POST")
            .uri("/v1/agent/stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"task": ""}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn valid_task_returns_sse_stream() {
        let state = test_state(vec![
            "Plan".into(),
            "Design".into(),
            "Approve".into(),
            "Code".into(),
            "Approve".into(),
        ]);
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/agent/stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"task": "Build an API"}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify content-type is SSE
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("text/event-stream"), "Expected SSE content-type, got: {ct}");
    }

    #[tokio::test]
    async fn semaphore_permit_released_after_completion() {
        let state = test_state(vec![
            "Plan".into(),
            "Design".into(),
            "Approve".into(),
            "Code".into(),
            "Approve".into(),
        ]);
        let sem = state.concurrency_limiter.clone();
        assert_eq!(sem.available_permits(), 2);

        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/agent/stream")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"task": "Test permit release"}"#))
            .unwrap();

        let _resp = app.oneshot(req).await.unwrap();

        // Give the spawned task time to finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Permit should be released after engine completes
        assert_eq!(sem.available_permits(), 2);
    }
}
