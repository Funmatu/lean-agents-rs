use crate::domain::tool_call::ToolCallRequest;
use crate::error::AppError;

/// Parsed output from an agent's response.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentOutput {
    /// Normal text response (may contain Approve/Reject directives).
    Speech(String),
    /// Tool call request extracted from JSON in the response.
    ToolCall(ToolCallRequest),
}

/// Parse an agent's raw output string into structured output.
///
/// Strategy:
/// 1. Try to find a JSON object with "action" and "query" fields.
/// 2. If found, extract it as a ToolCallRequest.
/// 3. Otherwise, treat the entire output as a Speech.
pub fn parse_agent_output(raw: &str) -> Result<AgentOutput, AppError> {
    let trimmed = raw.trim();

    // Try to find JSON object in the output
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = find_matching_brace(trimmed, start) {
            let json_str = &trimmed[start..=end];
            if let Ok(tool_call) = serde_json::from_str::<ToolCallRequest>(json_str) {
                if tool_call.action == "search" {
                    if tool_call.query.trim().is_empty() {
                        return Err(AppError::Parse("tool call query cannot be empty".to_string()));
                    }
                    if tool_call.query.len() > 10_000 {
                        return Err(AppError::Parse("tool call query too long".to_string()));
                    }
                    return Ok(AgentOutput::ToolCall(tool_call));
                }
            }
        }
    }

    if trimmed.is_empty() {
        return Err(AppError::Parse("empty agent output".to_string()));
    }

    Ok(AgentOutput::Speech(trimmed.to_string()))
}

/// Find the index of the matching closing brace for the opening brace at `start`.
fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in s[start..].char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                escape_next = true;
            }
            '"' => {
                in_string = !in_string;
            }
            '{' if !in_string => {
                depth += 1;
            }
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Check if a speech output contains an approval directive.
pub fn is_approval(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("approve") || lower.contains("lgtm")
}

/// Check if a speech output contains a rejection directive.
pub fn is_rejection(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("reject")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_speech() {
        let output = parse_agent_output("This is a design proposal.").unwrap();
        assert_eq!(
            output,
            AgentOutput::Speech("This is a design proposal.".to_string())
        );
    }

    #[test]
    fn parse_tool_call_json() {
        let raw = r#"I need to verify this. {"action": "search", "query": "Rust async trait"}"#;
        let output = parse_agent_output(raw).unwrap();
        match output {
            AgentOutput::ToolCall(tc) => {
                assert_eq!(tc.action, "search");
                assert_eq!(tc.query, "Rust async trait");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn parse_pure_json_tool_call() {
        let raw = r#"{"action": "search", "query": "tokio runtime"}"#;
        let output = parse_agent_output(raw).unwrap();
        assert!(matches!(output, AgentOutput::ToolCall(_)));
    }

    #[test]
    fn parse_non_search_json_as_speech() {
        let raw = r#"{"action": "other", "query": "test"}"#;
        let output = parse_agent_output(raw).unwrap();
        assert!(matches!(output, AgentOutput::Speech(_)));
    }

    #[test]
    fn parse_json_with_nested_braces() {
        let raw = r#"{"action": "search", "query": "struct { field: u32 }"}"#;
        let output = parse_agent_output(raw).unwrap();
        match output {
            AgentOutput::ToolCall(tc) => {
                assert_eq!(tc.query, "struct { field: u32 }");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn parse_empty_returns_error() {
        let result = parse_agent_output("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_whitespace_only_returns_error() {
        let result = parse_agent_output("   \n  ");
        assert!(result.is_err());
    }

    #[test]
    fn parse_malformed_json_as_speech() {
        let raw = r#"Here is some text with { broken json"#;
        let output = parse_agent_output(raw).unwrap();
        assert!(matches!(output, AgentOutput::Speech(_)));
    }

    #[test]
    fn approval_detection() {
        assert!(is_approval("I approve this design."));
        assert!(is_approval("LGTM, ship it!"));
        assert!(!is_approval("This needs more work."));
    }

    #[test]
    fn rejection_detection() {
        assert!(is_rejection("I reject this approach."));
        assert!(!is_rejection("Looks good to me."));
    }

    #[test]
    fn json_with_escaped_quotes() {
        let raw = r#"{"action": "search", "query": "he said \"hello\""}"#;
        let output = parse_agent_output(raw).unwrap();
        match output {
            AgentOutput::ToolCall(tc) => {
                assert_eq!(tc.action, "search");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn empty_query_rejected() {
        let raw = r#"{"action": "search", "query": ""}"#;
        assert!(parse_agent_output(raw).is_err());
    }

    #[test]
    fn whitespace_only_query_rejected() {
        let raw = r#"{"action": "search", "query": "   "}"#;
        assert!(parse_agent_output(raw).is_err());
    }
}
