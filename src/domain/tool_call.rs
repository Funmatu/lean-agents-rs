use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub action: String,
    pub query: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_request_deserialization() {
        let json = r#"{"action": "search", "query": "Rust async trait"}"#;
        let req: ToolCallRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.action, "search");
        assert_eq!(req.query, "Rust async trait");
    }

    #[test]
    fn tool_call_request_serialization_roundtrip() {
        let req = ToolCallRequest {
            action: "search".to_string(),
            query: "tokio runtime".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: ToolCallRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, deserialized);
    }
}
