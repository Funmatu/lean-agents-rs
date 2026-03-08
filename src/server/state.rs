use std::sync::Arc;
use dashmap::DashMap;
use tokio::sync::{mpsc, Semaphore};

use crate::client::{llm::LlmClient, search::SearchClient};
use crate::domain::state::WorkflowState;

/// Default maximum concurrent engine executions.
/// Protects GX10 UMA bandwidth and SGLang KV cache from saturation.
pub const DEFAULT_MAX_CONCURRENT_TASKS: usize = 4;

/// Shared application state for the axum server.
/// Cloneable (all fields are Arc-wrapped) for safe sharing across handlers.
#[derive(Clone)]
pub struct AppState {
    pub llm: Arc<dyn LlmClient>,
    pub search: Arc<dyn SearchClient>,
    pub concurrency_limiter: Arc<Semaphore>,
    pub active_interventions: Arc<DashMap<String, mpsc::Sender<(String, WorkflowState)>>>,
}

impl AppState {
    pub fn new(
        llm: Arc<dyn LlmClient>,
        search: Arc<dyn SearchClient>,
        max_concurrent_tasks: usize,
    ) -> Self {
        Self {
            llm,
            search,
            concurrency_limiter: Arc::new(Semaphore::new(max_concurrent_tasks)),
            active_interventions: Arc::new(DashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::llm::tests::MockLlmClient;
    use crate::client::search::tests::MockSearchClient;

    #[test]
    fn app_state_is_clone() {
        let state = AppState::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(MockSearchClient::new(vec![])),
            4,
        );
        let cloned = state.clone();
        // Both point to the same semaphore
        assert_eq!(
            Arc::strong_count(&state.concurrency_limiter),
            Arc::strong_count(&cloned.concurrency_limiter)
        );
    }

    #[tokio::test]
    async fn semaphore_limits_concurrency() {
        let state = AppState::new(
            Arc::new(MockLlmClient::new(vec![])),
            Arc::new(MockSearchClient::new(vec![])),
            2, // Only 2 permits
        );

        // Acquire both permits
        let _p1 = state.concurrency_limiter.acquire().await.unwrap();
        let _p2 = state.concurrency_limiter.acquire().await.unwrap();

        // Third acquire should not be immediately available
        assert_eq!(state.concurrency_limiter.available_permits(), 0);

        // Drop one permit — should free a slot
        drop(_p1);
        assert_eq!(state.concurrency_limiter.available_permits(), 1);
    }
}
