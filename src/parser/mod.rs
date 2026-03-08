use crate::domain::tool_call::ToolCallRequest;
use crate::error::AppError;

/// Parsed output from an agent's response.
/// Speech and ToolCall can coexist — the agent's reasoning (CoT) is preserved
/// even when a tool call is requested.
#[derive(Debug, PartialEq, Eq)]
pub struct ParsedOutput {
    /// Text surrounding the JSON (the agent's reasoning / CoT).
    /// For pure speech responses, this is the entire output.
    /// For tool calls, this is the text before and after the JSON block.
    pub speech: String,
    /// Extracted tool call request, if any.
    pub tool_call: Option<ToolCallRequest>,
}

/// Parse an agent's raw output string into structured output.
///
/// Strategy:
/// 1. Try to find a JSON object with "action":"search" and "query" fields.
/// 2. If found, extract JSON as a ToolCallRequest and join surrounding text as speech.
/// 3. Otherwise, treat the entire output as speech.
pub fn parse_agent_output(raw: &str) -> Result<ParsedOutput, AppError> {
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(AppError::Parse("empty agent output".to_string()));
    }

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

                    // Extract surrounding text as speech (CoT preservation)
                    let before = trimmed[..start].trim();
                    let after = trimmed[end + 1..].trim();
                    let speech = match (before.is_empty(), after.is_empty()) {
                        (true, true) => String::new(),
                        (false, true) => before.to_string(),
                        (true, false) => after.to_string(),
                        (false, false) => format!("{} {}", before, after),
                    };

                    return Ok(ParsedOutput {
                        speech,
                        tool_call: Some(tool_call),
                    });
                }
            }
        }
    }

    Ok(ParsedOutput {
        speech: trimmed.to_string(),
        tool_call: None,
    })
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
        assert_eq!(output.speech, "This is a design proposal.");
        assert!(output.tool_call.is_none());
    }

    #[test]
    fn parse_tool_call_with_cot() {
        let raw = r#"I need to verify this. {"action": "search", "query": "Rust async trait"}"#;
        let output = parse_agent_output(raw).unwrap();
        assert_eq!(output.speech, "I need to verify this.");
        let tc = output.tool_call.unwrap();
        assert_eq!(tc.action, "search");
        assert_eq!(tc.query, "Rust async trait");
    }

    #[test]
    fn parse_tool_call_with_cot_before_and_after() {
        let raw = r#"Let me check. {"action": "search", "query": "test"} I'll wait for results."#;
        let output = parse_agent_output(raw).unwrap();
        assert_eq!(output.speech, "Let me check. I'll wait for results.");
        assert!(output.tool_call.is_some());
    }

    #[test]
    fn parse_pure_json_tool_call() {
        let raw = r#"{"action": "search", "query": "tokio runtime"}"#;
        let output = parse_agent_output(raw).unwrap();
        assert!(output.speech.is_empty());
        assert!(output.tool_call.is_some());
    }

    #[test]
    fn parse_non_search_json_as_speech() {
        let raw = r#"{"action": "other", "query": "test"}"#;
        let output = parse_agent_output(raw).unwrap();
        assert_eq!(output.speech, raw);
        assert!(output.tool_call.is_none());
    }

    #[test]
    fn parse_json_with_nested_braces() {
        let raw = r#"{"action": "search", "query": "struct { field: u32 }"}"#;
        let output = parse_agent_output(raw).unwrap();
        let tc = output.tool_call.unwrap();
        assert_eq!(tc.query, "struct { field: u32 }");
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
        assert_eq!(output.speech, raw);
        assert!(output.tool_call.is_none());
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
        assert!(output.tool_call.is_some());
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
