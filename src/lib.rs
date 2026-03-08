pub mod domain;
pub mod client;
pub mod parser;
pub mod agents;
pub mod engine;

pub mod error {
    use thiserror::Error;

    #[derive(Debug, Error)]
    pub enum AppError {
        #[error("LLM client error: {0}")]
        LlmClient(String),

        #[error("Search client error: {0}")]
        SearchClient(String),

        #[error("Parse error: {0}")]
        Parse(String),

        #[error("Invalid state transition: {0}")]
        InvalidTransition(#[from] crate::domain::state::InvalidTransition),

        #[error("HTTP error: {0}")]
        Http(#[from] reqwest::Error),

        #[error("Configuration error: {0}")]
        Config(String),
    }
}
