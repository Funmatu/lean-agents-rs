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
    /// 2. Volatile context (temporary, inserted right after system)
    /// 3. Message history
    fn build_messages(&self, context: &ContextGraph) -> Vec<ChatMessage> {
        // Pre-allocate: 1 system + optional volatile + message history
        let history_len = context.messages().len();
        let volatile_present = context.volatile_context().is_some();
        let capacity = 1 + volatile_present as usize + history_len;
        let mut messages = Vec::with_capacity(capacity);

        // 1. System prompt (stable prefix for RadixAttention / Prefix Caching)
        //    This MUST remain identical across calls to maximize cache hits.
        messages.push(ChatMessage {
            role: "system".into(),
            content: self.system_prompt().to_string(),
        });

        // 2. Volatile context (ephemeral search results, dropped after use)
        //    Placed directly after system to keep prefix stable up to this point.
        if let Some(volatile) = context.volatile_context() {
            messages.push(ChatMessage {
                role: "system".into(),
                content: format!(
                    "[Temporary Reference — do not store]\n{}",
                    volatile
                ),
            });
        }

        // 3. Message history — we reference content via &str and format only
        //    the minimal wrapper. No .clone() of Message structs occurs here.
        let my_role = self.role();
        for msg in context.messages() {
            let role = if msg.sender == my_role { "assistant" } else { "user" };
            messages.push(ChatMessage {
                role: role.into(),
                content: format!("[{}] {}", msg.sender, msg.content),
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
    fn build_messages_with_volatile_context() {
        let agent = TestAgent;
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.set_volatile_context("Search result: Rust is great".to_string());

        let messages = agent.build_messages(&ctx);
        assert_eq!(messages.len(), 2); // system + volatile
        assert_eq!(messages[1].role, "system");
        assert!(messages[1].content.contains("Search result: Rust is great"));
        assert!(messages[1].content.contains("Temporary Reference"));
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
}
