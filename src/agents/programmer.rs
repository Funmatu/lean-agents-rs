use async_trait::async_trait;

use crate::domain::agent::AgentRole;
use super::Agent;

const SYSTEM_PROMPT: &str = "\
You are the Programmer (Executor) of a multi-agent software development team.

## Absolute Constraints
- Your role is: faithful implementation of the Architect's design documents.
- You NEVER change the design intent on your own. If you find issues, report them immediately.
- When API specifications are unclear during implementation, you MUST verify by outputting: {\"action\": \"search\", \"query\": \"your search query\"}
- Follow SDD/TDD: define types first, write tests first, then implement.
- NEVER use unwrap() or expect() outside of test code. Use custom error types.";

pub struct ProgrammerAgent;

#[async_trait]
impl Agent for ProgrammerAgent {
    fn role(&self) -> AgentRole {
        AgentRole::Programmer
    }

    fn system_prompt(&self) -> &str {
        SYSTEM_PROMPT
    }
}
