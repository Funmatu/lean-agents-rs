use serde::Serialize;

use super::agent::AgentRole;
use super::state::WorkflowState;

/// Events emitted by the Engine during workflow execution.
/// Consumed by API layer (SSE) and/or logging infrastructure.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    /// Workflow has started with the given task.
    WorkflowStarted { task: String },

    /// Ping event to keep SSE connection alive.
    Ping,

    /// State machine transitioned.
    StateChanged {
        from: WorkflowState,
        to: WorkflowState,
    },

    /// An agent is about to be invoked.
    AgentThinking { role: AgentRole },

    /// An agent produced a response.
    AgentSpoke { role: AgentRole, content: String },

    /// An agent requested and executed a tool call.
    ToolCallExecuted {
        role: AgentRole,
        action: String,
        query: String,
    },

    /// Workflow completed successfully.
    WorkflowCompleted,

    /// Workflow escalated (deadlock or parse failure).
    WorkflowEscalated {
        reason: String,
        task_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_event_serializes_to_tagged_json() {
        let event = EngineEvent::WorkflowStarted {
            task: "Build API".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"workflow_started""#));
        assert!(json.contains(r#""task":"Build API""#));
    }

    #[test]
    fn ping_event_serializes() {
        let event = EngineEvent::Ping;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"ping""#));
    }

    #[test]
    fn state_changed_event_serializes() {
        let event = EngineEvent::StateChanged {
            from: WorkflowState::Init,
            to: WorkflowState::Planning,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"state_changed""#));
    }

    #[test]
    fn agent_spoke_event_serializes() {
        let event = EngineEvent::AgentSpoke {
            role: AgentRole::Architect,
            content: "Design proposal".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"agent_spoke""#));
        assert!(json.contains(r#""role":"Architect""#));
    }

    #[test]
    fn tool_call_event_serializes() {
        let event = EngineEvent::ToolCallExecuted {
            role: AgentRole::Programmer,
            action: "search".into(),
            query: "tokio docs".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call_executed""#));
        assert!(json.contains(r#""query":"tokio docs""#));
    }

    #[test]
    fn escalated_event_serializes() {
        let event = EngineEvent::WorkflowEscalated {
            reason: "max iterations".into(),
            task_id: Some("1234-5678".into()),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"workflow_escalated""#));
        assert!(json.contains(r#""task_id":"1234-5678""#));
    }
}
