use super::agent::AgentRole;
use super::message::Message;
use super::state::{InvalidTransition, WorkflowState};

#[derive(Debug)]
pub struct ContextGraph {
    messages: Vec<Message>,
    state: WorkflowState,
    iteration: u32,
    volatile_context: Option<String>,
}

impl ContextGraph {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            state: WorkflowState::Init,
            iteration: 0,
            volatile_context: None,
        }
    }

    pub fn state(&self) -> &WorkflowState {
        &self.state
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn iteration(&self) -> u32 {
        self.iteration
    }

    pub fn volatile_context(&self) -> Option<&str> {
        self.volatile_context.as_deref()
    }

    /// Transition to a new state. Automatically clears volatile_context
    /// and increments iteration counter (except for ToolCalling transitions).
    pub fn transition_to(&mut self, target: WorkflowState) -> Result<(), InvalidTransition> {
        if !self.state.can_transition_to(&target) {
            return Err(InvalidTransition {
                from: self.state.clone(),
                to: target,
            });
        }

        // Always clear volatile context on state transition
        self.volatile_context.take();

        // Increment iteration only for non-transient transitions
        if !matches!(
            target,
            WorkflowState::ToolCalling { .. } | WorkflowState::CompressingContext { .. }
        ) {
            self.iteration += 1;
        }

        self.state = target;
        Ok(())
    }

    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Set volatile context (used to store search results for the next turn).
    pub fn set_volatile_context(&mut self, context: String) {
        self.volatile_context = Some(context);
    }

    /// Explicitly clear volatile context.
    pub fn clear_volatile_context(&mut self) {
        self.volatile_context.take();
    }

    /// Calculate the total character length of all message contents.
    pub fn total_content_length(&self) -> usize {
        self.messages.iter().map(|m| m.content.len()).sum()
    }

    /// Reset message history with a summary checkpoint.
    /// Clears all messages and inserts a single system summary at the head.
    pub fn reset_with_summary(&mut self, summary: String) {
        self.messages.clear();
        self.messages.push(Message::new(
            AgentRole::System,
            &format!("[System Checkpoint Summary]\n{}", summary),
        ));
    }
}

impl Default for ContextGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::AgentRole;

    #[test]
    fn new_context_graph_is_init() {
        let ctx = ContextGraph::new();
        assert_eq!(*ctx.state(), WorkflowState::Init);
        assert_eq!(ctx.iteration(), 0);
        assert!(ctx.messages().is_empty());
        assert!(ctx.volatile_context().is_none());
    }

    #[test]
    fn valid_transition_clears_volatile_context() {
        let mut ctx = ContextGraph::new();
        ctx.set_volatile_context("search results".to_string());
        assert!(ctx.volatile_context().is_some());

        ctx.transition_to(WorkflowState::Planning).unwrap();
        assert!(ctx.volatile_context().is_none());
        assert_eq!(ctx.iteration(), 1);
    }

    #[test]
    fn invalid_transition_returns_error() {
        let mut ctx = ContextGraph::new();
        let result = ctx.transition_to(WorkflowState::Completed);
        assert!(result.is_err());
        // State should remain unchanged
        assert_eq!(*ctx.state(), WorkflowState::Init);
    }

    #[test]
    fn tool_calling_does_not_increment_iteration() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.transition_to(WorkflowState::Designing).unwrap();
        let iter_before = ctx.iteration();

        ctx.transition_to(WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Designing),
        })
        .unwrap();

        assert_eq!(ctx.iteration(), iter_before);
    }

    #[test]
    fn tool_calling_clears_volatile_context() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.transition_to(WorkflowState::Designing).unwrap();

        ctx.set_volatile_context("old search".to_string());
        ctx.transition_to(WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Designing),
        })
        .unwrap();

        assert!(ctx.volatile_context().is_none());
    }

    #[test]
    fn return_from_tool_calling() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.transition_to(WorkflowState::Designing).unwrap();
        ctx.transition_to(WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Designing),
        })
        .unwrap();

        // Set search results as volatile context
        ctx.set_volatile_context("fresh search results".to_string());

        // Return to Designing
        ctx.transition_to(WorkflowState::Designing).unwrap();

        // Volatile context should be cleared on transition
        assert!(ctx.volatile_context().is_none());
        assert_eq!(*ctx.state(), WorkflowState::Designing);
    }

    #[test]
    fn add_message_preserves_order() {
        let mut ctx = ContextGraph::new();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "plan"));
        ctx.add_message(Message::new(AgentRole::Architect, "design"));

        assert_eq!(ctx.messages().len(), 2);
        assert_eq!(ctx.messages()[0].sender, AgentRole::Orchestrator);
        assert_eq!(ctx.messages()[1].sender, AgentRole::Architect);
    }

    #[test]
    fn full_workflow_lifecycle() {
        let mut ctx = ContextGraph::new();

        // Init -> Planning -> Designing -> Implementing -> Reviewing -> Completed
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "Start planning"));

        ctx.transition_to(WorkflowState::Designing).unwrap();
        ctx.add_message(Message::new(AgentRole::Architect, "Design ready"));

        ctx.transition_to(WorkflowState::Implementing).unwrap();
        ctx.add_message(Message::new(AgentRole::Programmer, "Code done"));

        ctx.transition_to(WorkflowState::Reviewing).unwrap();
        ctx.add_message(Message::new(AgentRole::DevilsAdvocate, "LGTM"));

        ctx.transition_to(WorkflowState::Completed).unwrap();

        assert_eq!(*ctx.state(), WorkflowState::Completed);
        assert_eq!(ctx.iteration(), 5);
        assert_eq!(ctx.messages().len(), 4);
    }

    #[test]
    fn total_content_length_sums_all_messages() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "hello")); // 5
        ctx.add_message(Message::new(AgentRole::Architect, "world!")); // 6
        assert_eq!(ctx.total_content_length(), 11);
    }

    #[test]
    fn reset_with_summary_clears_and_inserts_checkpoint() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.add_message(Message::new(AgentRole::Orchestrator, "msg1"));
        ctx.add_message(Message::new(AgentRole::Architect, "msg2"));
        assert_eq!(ctx.messages().len(), 2);

        ctx.reset_with_summary("This is a summary".to_string());
        assert_eq!(ctx.messages().len(), 1);
        assert_eq!(ctx.messages()[0].sender, AgentRole::System);
        assert!(ctx.messages()[0].content.contains("[System Checkpoint Summary]"));
        assert!(ctx.messages()[0].content.contains("This is a summary"));
    }

    #[test]
    fn compressing_context_does_not_increment_iteration() {
        let mut ctx = ContextGraph::new();
        ctx.transition_to(WorkflowState::Planning).unwrap();
        ctx.transition_to(WorkflowState::Designing).unwrap();
        let iter_before = ctx.iteration();

        ctx.transition_to(WorkflowState::CompressingContext {
            return_to: Box::new(WorkflowState::Designing),
        })
        .unwrap();
        assert_eq!(ctx.iteration(), iter_before);

        // Return to Designing
        ctx.transition_to(WorkflowState::Designing).unwrap();
        assert_eq!(ctx.iteration(), iter_before + 1);
    }

    #[test]
    fn escalation_from_any_phase() {
        for start_state in [
            WorkflowState::Planning,
            WorkflowState::Designing,
            WorkflowState::Implementing,
            WorkflowState::Reviewing,
        ] {
            let mut ctx = ContextGraph::new();
            ctx.transition_to(WorkflowState::Planning).unwrap();
            if start_state != WorkflowState::Planning {
                ctx.transition_to(WorkflowState::Designing).unwrap();
            }
            if start_state == WorkflowState::Implementing
                || start_state == WorkflowState::Reviewing
            {
                ctx.transition_to(WorkflowState::Implementing).unwrap();
            }
            if start_state == WorkflowState::Reviewing {
                ctx.transition_to(WorkflowState::Reviewing).unwrap();
            }
            ctx.transition_to(WorkflowState::Escalated).unwrap();
            assert_eq!(*ctx.state(), WorkflowState::Escalated);
        }
    }
}
