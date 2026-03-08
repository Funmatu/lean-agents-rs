use std::env;
use std::sync::Arc;

use lean_agents_rs::client::llm::SgLangClient;
use lean_agents_rs::client::search::TavilyClient;
use lean_agents_rs::server::router::build_router;
use lean_agents_rs::server::state::{AppState, DEFAULT_MAX_CONCURRENT_TASKS, DEFAULT_MAX_CONTEXT_LENGTH};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let sglang_url = env::var("SGLANG_URL").unwrap_or_else(|_| {
        tracing::warn!("SGLANG_URL not set, using default http://localhost:30000");
        "http://localhost:30000".to_string()
    });

    let model = env::var("SGLANG_MODEL").unwrap_or_else(|_| "Qwen3.5-27B".to_string());

    let tavily_key = env::var("TAVILY_API_KEY").unwrap_or_else(|_| {
        tracing::warn!("TAVILY_API_KEY not set, search functionality will fail");
        String::new()
    });

    let max_concurrent: usize = env::var("MAX_CONCURRENT_TASKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONCURRENT_TASKS);

    let max_context_length: usize = env::var("MAX_CONTEXT_LENGTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_CONTEXT_LENGTH);

    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);

    let llm = Arc::new(SgLangClient::new(&sglang_url, &model));
    let search = Arc::new(TavilyClient::new(&tavily_key));
    let state = AppState::new(llm, search, max_concurrent, max_context_length);

    let app = build_router(state)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let bind_addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .expect("Failed to bind TCP listener");

    tracing::info!("lean-agents-rs API server starting");
    tracing::info!("  Endpoint: http://{bind_addr}");
    tracing::info!("  SGLang:   {sglang_url}");
    tracing::info!("  Model:    {model}");
    tracing::info!("  Max concurrent tasks: {max_concurrent}");
    tracing::info!("  Max context length:   {max_context_length}");

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
