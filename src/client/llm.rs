use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// A single message in the OpenAI-compatible chat format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Request body for the chat completions endpoint.
#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// When true, the response is streamed as SSE chunks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

/// A single choice in the non-streaming completion response.
#[derive(Debug, Deserialize)]
pub struct ChatChoice {
    pub message: ChatMessage,
}

/// Non-streaming response body from the chat completions endpoint.
#[derive(Debug, Deserialize)]
pub struct ChatCompletionResponse {
    pub choices: Vec<ChatChoice>,
}

/// Delta content in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct StreamDelta {
    #[serde(default)]
    pub content: Option<String>,
}

/// A single choice in a streaming chunk.
#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
}

/// Streaming chunk response.
#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<String, AppError>;
}

/// SGLang-compatible LLM client using OpenAI-compatible API.
/// Uses SSE streaming to avoid buffering the entire response in UMA.
pub struct SgLangClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    max_retries: u32,
}

impl SgLangClient {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            model: model.into(),
            max_retries: 3,
        }
    }

    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Consume an SSE stream, appending delta content chunks into a single String.
    /// Each SSE line is `data: <json>` or `data: [DONE]`.
    /// This avoids holding the entire response in a single allocation.
    async fn consume_stream(
        &self,
        resp: reqwest::Response,
    ) -> Result<String, AppError> {
        let mut result = String::new();
        let mut stream = resp.bytes_stream();

        // We accumulate raw bytes, then parse line-by-line.
        // bytes_stream yields small chunks so we never hold the full response.
        let mut buffer = String::new();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AppError::LlmClient(e.to_string()))?;
            buffer.push_str(
                &String::from_utf8_lossy(&chunk),
            );

            // Process complete lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return if result.is_empty() {
                            Err(AppError::LlmStreamError(
                                "stream completed with no content (possible context overflow)".to_string(),
                            ))
                        } else {
                            Ok(result)
                        };
                    }

                    match serde_json::from_str::<StreamChunk>(data) {
                        Ok(chunk) => {
                            if let Some(choice) = chunk.choices.into_iter().next() {
                                if let Some(content) = choice.delta.content {
                                    result.push_str(&content);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to parse stream chunk: {}", e);
                        }
                    }
                }
            }
        }

        // Stream ended without [DONE] — return what we have
        if result.is_empty() {
            Err(AppError::LlmStreamError(
                "stream ended unexpectedly with no content (possible context overflow)".to_string(),
            ))
        } else {
            Ok(result)
        }
    }
}

#[async_trait]
impl LlmClient for SgLangClient {
    async fn chat_completion(
        &self,
        mut request: ChatCompletionRequest,
    ) -> Result<String, AppError> {
        request.model = self.model.clone();
        request.stream = Some(true);
        let url = format!("{}/v1/chat/completions", self.base_url);

        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(100 * 2u64.pow(attempt - 1)))
                    .await;
            }

            match self.http.post(&url)
                .timeout(std::time::Duration::from_secs(120))
                .json(&request)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_success() {
                        return self.consume_stream(resp).await;
                    }
                    let status = resp.status();
                    let text = match resp.text().await {
                        Ok(t) => t,
                        Err(_) => "[unable to read response body]".to_string(),
                    };
                    last_err = Some(AppError::LlmClient(format!(
                        "HTTP {status}: {text}"
                    )));
                }
                Err(e) => {
                    last_err = Some(AppError::LlmClient(e.to_string()));
                }
            }
        }

        Err(last_err.unwrap_or_else(|| AppError::LlmClient("unknown error".to_string())))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Mock LLM client for testing.
    pub struct MockLlmClient {
        responses: Vec<String>,
        call_count: Arc<AtomicU32>,
        fail_count: u32,
    }

    impl MockLlmClient {
        pub fn new(responses: Vec<String>) -> Self {
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
    impl LlmClient for MockLlmClient {
        async fn chat_completion(
            &self,
            _request: ChatCompletionRequest,
        ) -> Result<String, AppError> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            if count < self.fail_count {
                return Err(AppError::LlmClient("mock failure".to_string()));
            }
            let idx = (count - self.fail_count) as usize;
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| AppError::LlmClient("no more mock responses".to_string()))
        }
    }

    #[tokio::test]
    async fn mock_llm_returns_responses_in_order() {
        let client = MockLlmClient::new(vec!["first".into(), "second".into()]);
        let req = || ChatCompletionRequest {
            model: String::new(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
        };

        let r1 = client.chat_completion(req()).await.unwrap();
        assert_eq!(r1, "first");

        let r2 = client.chat_completion(req()).await.unwrap();
        assert_eq!(r2, "second");
    }

    #[tokio::test]
    async fn mock_llm_with_failures() {
        let client = MockLlmClient::new(vec!["success".into()]).with_failures(2);
        let req = || ChatCompletionRequest {
            model: String::new(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            stream: None,
        };

        assert!(client.chat_completion(req()).await.is_err());
        assert!(client.chat_completion(req()).await.is_err());
        let result = client.chat_completion(req()).await.unwrap();
        assert_eq!(result, "success");
        assert_eq!(client.call_count(), 3);
    }
}
