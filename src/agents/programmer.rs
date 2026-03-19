use async_trait::async_trait;

use crate::domain::agent::AgentRole;
use super::Agent;

const SYSTEM_PROMPT: &str = "\
You are the Programmer (Executor) of a multi-agent software development team.

## Absolute Constraints
- Your role is: faithful implementation of the Architect's design documents.
- You NEVER change the design intent on your own. If you find issues, report them immediately.
- Follow SDD/TDD: define types first, write tests first, then implement.
- NEVER use unwrap() or expect() outside of test code. Use custom error types.
- When ANY API specification, function signature, or library behavior is unclear during implementation, you MUST search before writing code. Do NOT guess — verify first.

## Tool Call Format (MANDATORY)
When you need external information, you MUST first explain your reasoning in plain text, then output the JSON on the same line or immediately after:
Example: \"I need to check the exact method signature. {\"action\": \"search\", \"query\": \"your search query\"}\"";

pub struct ProgrammerAgent;

#[async_trait]
impl Agent for ProgrammerAgent {
    fn role(&self) -> AgentRole {
        AgentRole::Programmer
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }

    /// Low temperature: code generation demands precision and correctness
    /// over creativity. Minimizes hallucinated APIs and syntax errors.
    fn temperature(&self) -> f32 { 0.3 }

    /// Higher token budget: code output is inherently longer than prose.
    /// Prevents truncated implementations.
    fn max_tokens(&self) -> u32 { 4096 }
}
