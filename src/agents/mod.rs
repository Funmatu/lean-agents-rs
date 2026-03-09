pub mod orchestrator;
pub mod architect;
pub mod programmer;
pub mod devils_advocate;

use async_trait::async_trait;

use crate::client::llm::{ChatCompletionRequest, ChatMessage, LlmClient};
use crate::domain::agent::AgentRole;
use crate::domain::context::ContextGraph;
use crate::error::AppError;

/// Core agent trait. Each agent builds a prompt from the context graph
/// and delegates to the LLM client.
#[async_trait]
pub trait Agent: Send + Sync {
    fn role(&self) -> AgentRole;
    fn system_prompt(&self) -> &str;

    /// Build the full message array for the LLM, respecting RadixAttention:
    /// 1. System prompt (immutable prefix — cached by SGLang)
    /// 2. Message history (grows monotonically — prefix-cacheable across turns)
    /// 3. Volatile context (ephemeral, tail position — never invalidates prefix cache)
    fn build_messages(&self, context: &ContextGraph) -> Vec<ChatMessage> {
        let history_len = context.messages().len();
        let volatile_present = context.volatile_context().is_some();
        let capacity = 1 + history_len + volatile_present as usize;
        let mut messages = Vec::with_capacity(capacity);

        // 1. System prompt (stable prefix for RadixAttention / Prefix Caching)
        messages.push(ChatMessage {
            role: "system".into(),
            content: self.system_prompt().to_string(),
        });

        // 2. Message history — monotonically growing, maximizes prefix cache hits
        let my_role = self.role();
        for msg in context.messages() {
            let role = if msg.sender == my_role { "assistant" } else { "user" };
            messages.push(ChatMessage {
                role: role.into(),
                content: format!("[{}] {}", msg.sender, msg.content),
            });
        }

        // 3. Volatile context (ephemeral search results, tail position)
        //    Using "user" role to avoid breaking the system prompt prefix cache.
        //    Dropped after one turn so it never accumulates in history.
        if let Some(volatile) = context.volatile_context() {
            messages.push(ChatMessage {
                role: "user".into(),
                content: format!(
                    "[Temporary Reference — do not store]\n{}",
                    volatile
                ),
            });
        }

        // llama.cpp compatibility: prevent assistant prefill.
        // llama.cpp rejects requests where the last message has role "assistant"
        // when enable_thinking is active (HTTP 400: "Assistant response prefill
        // is incompatible with enable_thinking."). We append a user-role trigger
        // message to ensure the array always ends with "user".
        if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
            messages.push(ChatMessage {
                role: "user".into(),
                content: "Continue your analysis based on the context above.".into(),
            });
        }

        messages
    }

    /// Execute the agent: build prompt, call LLM, return raw response.
    async fn execute(
        &self,
        context: &ContextGraph,
        llm: &dyn LlmClient,
    ) -> Result<String, AppError> {
        let messages = self.build_messages(context);
        let request = ChatCompletionRequest {
            model: String::new(), // Overridden by client
            messages,
            temperature: Some(0.7),
            max_tokens: Some(2048),
            stream: None, // Controlled by client implementation
        };
        llm.chat_completion(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::context::ContextGraph;
    use crate::domain::message::Message;
    use crate::domain::state::WorkflowState;

    struct TestAgent;

    #[async_trait]
    impl Agent for TestAgent {
        fn role(&self) -> AgentRole {
            AgentRole::Architect
        }
        fn system_prompt(&self) -> &str {
            "You are a test agent."
        }
    }

    #[test]
    fn build_messages_without_volatile_context() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "Plan this"));

        let messages = agent.build_messages(&ctx);
        assert_eq!(messages.len(), 2); // system + 1 history
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, "You are a test agent.");
        assert_eq!(messages[1].role, "user"); // Orchestrator != Architect
    }

    #[test]
    fn build_messages_with_volatile_context_at_end() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "Plan this"));
        ctx.set_volatile_context("Search result: Rust is great".to_string());

        let messages = agent.build_messages(&ctx);
        // system + 1 history + volatile at end
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, "You are a test agent.");
        // History comes second (prefix-cacheable)
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("Plan this"));
        // Volatile context is LAST (tail position, never breaks prefix cache)
        let last = &messages[2];
        assert_eq!(last.role, "user");
        assert!(last.content.contains("Search result: Rust is great"));
        assert!(last.content.contains("Temporary Reference"));
    }

    #[test]
    fn volatile_only_no_history() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.set_volatile_context("some data".to_string());

        let messages = agent.build_messages(&ctx);
        assert_eq!(messages.len(), 2); // system + volatile
        assert_eq!(messages[0].role, "system");
        // Volatile is last even with no history
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("some data"));
    }

    #[test]
    fn own_messages_are_assistant_role() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Architect, "My proposal"));
        ctx.add_message(Message::new(AgentRole::Programmer, "Understood"));

        let messages = agent.build_messages(&ctx);
        // system + 2 history messages
        assert_eq!(messages[1].role, "assistant"); // Architect's own message
        assert_eq!(messages[2].role, "user"); // Programmer's message
    }

    /// llama.cpp compatibility: when the only history message belongs to the
    /// agent itself, the raw mapping would produce a trailing "assistant" role,
    /// which llama.cpp rejects with HTTP 400 ("Assistant response prefill is
    /// incompatible with enable_thinking."). The compatibility layer must
    /// append a "user" trigger message to prevent this.
    #[test]
    fn test_build_messages_prevents_assistant_prefill() {
        let agent = TestAgent; // role = Architect
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();

        // Only the agent's own message exists → would map to role "assistant"
        ctx.add_message(Message::new(AgentRole::Architect, "Initial design draft"));

        let messages = agent.build_messages(&ctx);

        // The last message MUST NOT be "assistant" (llama.cpp constraint)
        let last = messages.last().unwrap();
        assert_ne!(
            last.role, "assistant",
            "build_messages must never end with assistant role (llama.cpp compatibility)"
        );
        assert_eq!(last.role, "user");

        // Verify structure: system + assistant(own) + user(trigger)
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[2].role, "user");
    }

    /// Verify the compatibility layer also activates when multiple consecutive
    /// messages from the same agent end the history.
    #[test]
    fn test_build_messages_prevents_assistant_prefill_multiple_own_messages() {
        let agent = TestAgent; // role = Architect
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();

        ctx.add_message(Message::new(AgentRole::Architect, "Draft v1"));
        ctx.add_message(Message::new(AgentRole::Architect, "Draft v2 revision"));

        let messages = agent.build_messages(&ctx);

        let last = messages.last().unwrap();
        assert_ne!(last.role, "assistant");
        assert_eq!(last.role, "user");
    }

    /// When volatile context is present, it's already a "user" message at the
    /// tail, so the compatibility patch should NOT fire (no extra message).
    #[test]
    fn test_volatile_context_prevents_assistant_prefill_naturally() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();

        // Own message → would be "assistant"
        ctx.add_message(Message::new(AgentRole::Architect, "My analysis"));
        // But volatile context adds a "user" message at tail
        ctx.set_volatile_context("Search results here".to_string());

        let messages = agent.build_messages(&ctx);

        // system + assistant(own) + user(volatile) — no extra trigger needed
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].role, "user");
        assert!(messages[2].content.contains("Search results here"));
    }
}
