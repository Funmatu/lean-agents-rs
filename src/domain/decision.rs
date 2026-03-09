/// Structured representation of a reviewer's decision.
///
/// Parsed from LLM output using a "last keyword wins" strategy:
/// the final occurrence of an approve/reject keyword in the text
/// determines the decision, preventing false positives when both
/// keywords appear in the same response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The reviewer explicitly approves.
    Approve,
    /// The reviewer explicitly rejects, with a reason extracted from the text.
    Reject(String),
    /// The reviewer requests a revision (treated as a soft reject).
    RequestRevision(String),
}

/// Keyword kinds used during scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeywordKind {
    Approve,
    Reject,
    Revise,
}

/// A keyword match with its position in the text.
struct KeywordMatch {
    kind: KeywordKind,
    /// Byte offset where the keyword starts in the lowercased text.
    position: usize,
}

/// Approval keywords (checked in lowercase).
const APPROVE_KEYWORDS: &[&str] = &["approve", "lgtm"];

/// Rejection keywords (checked in lowercase).
const REJECT_KEYWORDS: &[&str] = &["reject"];

/// Revision-request keywords (checked in lowercase).
const REVISE_KEYWORDS: &[&str] = &["revise", "revision requested", "request revision"];

/// Parse a reviewer's raw output text into a structured `ReviewDecision`.
///
/// Strategy ("last keyword wins"):
/// 1. Scan the text for all occurrences of approve/reject/revise keywords.
/// 2. The keyword with the **largest byte offset** (i.e., last in text) wins.
/// 3. If no keywords are found, default to `RequestRevision` with the full text
///    as the reason (ambiguous output is never treated as approval).
///
/// Reason extraction:
/// - For `Reject` and `RequestRevision`, the text after the winning keyword
///   (up to the end of the sentence or 200 chars) is captured as the reason.
/// - If no meaningful reason text follows the keyword, the full input is used.
pub fn parse_review_decision(text: &str) -> ReviewDecision {
    let lower = text.to_lowercase();

    let mut matches: Vec<KeywordMatch> = Vec::new();

    // Scan for all keyword occurrences
    for &kw in APPROVE_KEYWORDS {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(kw) {
            let abs_pos = start + pos;
            // Ensure word boundary: keyword must not be part of a larger word
            if is_word_boundary(&lower, abs_pos, kw.len()) {
                matches.push(KeywordMatch {
                    kind: KeywordKind::Approve,
                    position: abs_pos,
                });
            }
            start = abs_pos + kw.len();
        }
    }

    for &kw in REJECT_KEYWORDS {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(kw) {
            let abs_pos = start + pos;
            if is_word_boundary(&lower, abs_pos, kw.len()) {
                matches.push(KeywordMatch {
                    kind: KeywordKind::Reject,
                    position: abs_pos,
                });
            }
            start = abs_pos + kw.len();
        }
    }

    for &kw in REVISE_KEYWORDS {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(kw) {
            let abs_pos = start + pos;
            if is_word_boundary(&lower, abs_pos, kw.len()) {
                matches.push(KeywordMatch {
                    kind: KeywordKind::Revise,
                    position: abs_pos,
                });
            }
            start = abs_pos + kw.len();
        }
    }

    // No keywords found → ambiguous, treat as revision request (never auto-approve)
    if matches.is_empty() {
        return ReviewDecision::RequestRevision(truncate_reason(text.trim()));
    }

    // Last keyword wins
    let winner = matches
        .iter()
        .max_by_key(|m| m.position)
        .expect("matches is non-empty");

    match winner.kind {
        KeywordKind::Approve => ReviewDecision::Approve,
        KeywordKind::Reject => {
            let reason = extract_reason_after(text, winner.position);
            ReviewDecision::Reject(reason)
        }
        KeywordKind::Revise => {
            let reason = extract_reason_after(text, winner.position);
            ReviewDecision::RequestRevision(reason)
        }
    }
}

/// Check if the keyword at `pos` with length `len` is at a word boundary.
fn is_word_boundary(lower: &str, pos: usize, len: usize) -> bool {
    let before_ok = pos == 0
        || lower.as_bytes().get(pos - 1).map_or(true, |&b| {
            !b.is_ascii_alphanumeric() && b != b'_'
        });
    let after_pos = pos + len;
    let after_ok = after_pos >= lower.len()
        || lower.as_bytes().get(after_pos).map_or(true, |&b| {
            !b.is_ascii_alphanumeric() && b != b'_'
        });
    before_ok && after_ok
}

/// Extract reason text after the keyword position. Takes up to the next
/// sentence boundary or 200 characters, whichever comes first.
fn extract_reason_after(text: &str, keyword_pos: usize) -> String {
    // Find the end of the keyword (skip to next non-alpha char)
    let after_keyword = &text[keyword_pos..];
    let rest = after_keyword
        .find(|c: char| !c.is_ascii_alphabetic() && c != '_')
        .map(|i| &after_keyword[i..])
        .unwrap_or("");

    let rest = rest.trim_start_matches(|c: char| c == ':' || c == '-' || c == '.' || c.is_whitespace());

    if rest.is_empty() {
        return truncate_reason(text.trim());
    }

    truncate_reason(rest.trim())
}

/// Truncate reason to at most 200 characters.
fn truncate_reason(s: &str) -> String {
    if s.len() <= 200 {
        s.to_string()
    } else {
        format!("{}...", &s[..197])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_approve() {
        let d = parse_review_decision("I approve this design.");
        assert_eq!(d, ReviewDecision::Approve);
    }

    #[test]
    fn simple_lgtm() {
        let d = parse_review_decision("LGTM, ship it!");
        assert_eq!(d, ReviewDecision::Approve);
    }

    #[test]
    fn simple_reject() {
        let d = parse_review_decision("I reject this approach.");
        assert!(matches!(d, ReviewDecision::Reject(_)));
    }

    #[test]
    fn reject_with_reason() {
        let d = parse_review_decision("I reject this: the API is insecure.");
        match d {
            ReviewDecision::Reject(reason) => {
                assert!(reason.contains("the API is insecure"));
            }
            other => panic!("expected Reject, got {:?}", other),
        }
    }

    /// The critical bug scenario: text says "approve" early but "reject" at the end.
    /// Last keyword wins → Reject.
    #[test]
    fn approve_then_reject_last_wins() {
        let text = "This is a great design and I would normally approve it, \
                     but after careful review I must reject it due to security concerns.";
        let d = parse_review_decision(text);
        assert!(matches!(d, ReviewDecision::Reject(_)), "got {:?}", d);
    }

    /// Opposite scenario: "reject" early, "approve" at the end → Approve.
    #[test]
    fn reject_then_approve_last_wins() {
        let text = "My earlier rejection of the design is withdrawn. \
                     After seeing the revisions, I approve.";
        let d = parse_review_decision(text);
        assert_eq!(d, ReviewDecision::Approve);
    }

    /// Japanese-style mixed output from the implementation plan scenario:
    /// "これは素晴らしい設計ですが、以下の点でRejectします。"
    #[test]
    fn japanese_mixed_reject() {
        let text = "これは素晴らしい設計ですが、以下の点でRejectします。セキュリティが不十分です。";
        let d = parse_review_decision(text);
        assert!(matches!(d, ReviewDecision::Reject(_)), "got {:?}", d);
    }

    #[test]
    fn no_keywords_defaults_to_revision() {
        let text = "This needs more work. The design is incomplete.";
        let d = parse_review_decision(text);
        assert!(matches!(d, ReviewDecision::RequestRevision(_)));
    }

    #[test]
    fn revision_requested() {
        let text = "Please revise the error handling section.";
        let d = parse_review_decision(text);
        assert!(matches!(d, ReviewDecision::RequestRevision(_)));
    }

    #[test]
    fn approve_as_substring_not_matched() {
        // "disapprove" should not match "approve" due to word boundary check
        let text = "I disapprove of this approach entirely.";
        let d = parse_review_decision(text);
        // "disapprove" contains "approve" but word boundary check prevents match
        // No valid keywords → defaults to RequestRevision
        assert!(
            matches!(d, ReviewDecision::RequestRevision(_)),
            "got {:?}",
            d
        );
    }

    #[test]
    fn empty_input() {
        let d = parse_review_decision("");
        assert!(matches!(d, ReviewDecision::RequestRevision(_)));
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            parse_review_decision("APPROVE"),
            ReviewDecision::Approve
        );
        assert!(matches!(
            parse_review_decision("REJECT this"),
            ReviewDecision::Reject(_)
        ));
    }

    #[test]
    fn multiple_reject_keywords() {
        let text = "I reject point A. I also reject point B for being unsafe.";
        let d = parse_review_decision(text);
        match d {
            ReviewDecision::Reject(reason) => {
                assert!(reason.contains("being unsafe"));
            }
            other => panic!("expected Reject, got {:?}", other),
        }
    }
}
