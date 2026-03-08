use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[async_trait]
pub trait SearchClient: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError>;
}

/// Tavily API search client.
pub struct TavilyClient {
    http: reqwest::Client,
    api_key: String,
    max_retries: u32,
}

#[derive(Serialize)]
struct TavilyRequest<'a> {
    query: &'a str,
    search_depth: &'a str,
    max_results: u32,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
}

impl TavilyClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
            max_retries: 3,
        }
    }

    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }
}

#[async_trait]
impl SearchClient for TavilyClient {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let url = "https://api.tavily.com/search";
        let body = TavilyRequest {
            query,
            search_depth: "basic",
            max_results: 3,
        };

        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(100 * 2u64.pow(attempt - 1)))
                    .await;
            }

            match self
                .http
                .post(url)
                .timeout(std::time::Duration::from_secs(30))
                .header("Content-Type", "application/json")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let tavily_resp: TavilyResponse = resp
                            .json()
                            .await
                            .map_err(|e| AppError::SearchClient(e.to_string()))?;
                        return Ok(tavily_resp
                            .results
                            .into_iter()
                            .map(|r| SearchResult {
                                title: r.title,
                                url: r.url,
                                snippet: r.content,
                            })
                            .collect());
                    }
                    let status = resp.status();
                    let text = match resp.text().await {
                        Ok(t) => t,
                        Err(_) => "[unable to read response body]".to_string(),
                    };
                    last_err = Some(AppError::SearchClient(format!("HTTP {status}: {text}")));
                }
                Err(e) => {
                    last_err = Some(AppError::SearchClient(e.to_string()));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| AppError::SearchClient("unknown error".to_string())))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Mock search client for testing.
    pub struct MockSearchClient {
        responses: Vec<Vec<SearchResult>>,
        call_count: Arc<AtomicU32>,
        fail_count: u32,
    }

    impl MockSearchClient {
        pub fn new(responses: Vec<Vec<SearchResult>>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicU32::new(0)),
                fail_count: 0,
            }
        }

        pub fn with_failures(mut self, fail_count: u32) -> Self {
            self.fail_count = fail_count;
            self
        }

        pub fn call_count(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SearchClient for MockSearchClient {
        async fn search(&self, _query: &str) -> Result<Vec<SearchResult>, AppError> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count < self.fail_count {
                return Err(AppError::SearchClient("mock failure".to_string()));
            }
            let idx = (count - self.fail_count) as usize;
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| AppError::SearchClient("no more mock responses".to_string()))
        }
    }

    #[tokio::test]
    async fn mock_search_returns_results() {
        let results = vec![SearchResult {
            title: "Rust docs".into(),
            url: "https://doc.rust-lang.org".into(),
            snippet: "The Rust programming language".into(),
        }];
        let client = MockSearchClient::new(vec![results.clone()]);

        let got = client.search("rust").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Rust docs");
    }

    #[tokio::test]
    async fn mock_search_with_failures_then_success() {
        let results = vec![SearchResult {
            title: "Result".into(),
            url: "https://example.com".into(),
            snippet: "example".into(),
        }];
        let client = MockSearchClient::new(vec![results]).with_failures(2);

        assert!(client.search("test").await.is_err());
        assert!(client.search("test").await.is_err());
        let got = client.search("test").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(client.call_count(), 3);
    }

    #[tokio::test]
    async fn mock_search_exhausted_returns_error() {
        let client = MockSearchClient::new(vec![]);
        assert!(client.search("test").await.is_err());
    }
}
