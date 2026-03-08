use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkflowState {
    Init,
    Planning,
    Designing,
    Implementing,
    Reviewing,
    ToolCalling {
        return_to: Box<WorkflowState>,
    },
    Completed,
    Escalated,
}

#[derive(Debug, Error)]
#[error("invalid state transition from {from:?} to {to:?}")]
pub struct InvalidTransition {
    pub from: WorkflowState,
    pub to: WorkflowState,
}

impl WorkflowState {
    pub fn can_transition_to(&self, target: &WorkflowState) -> bool {
        match (self, target) {
            // Init can go to Planning
            (WorkflowState::Init, WorkflowState::Planning) => true,

            // Planning can go to Designing or Escalated
            (WorkflowState::Planning, WorkflowState::Designing) => true,
            (WorkflowState::Planning, WorkflowState::Escalated) => true,

            // Designing can go to Implementing, ToolCalling, or Escalated
            (WorkflowState::Designing, WorkflowState::Implementing) => true,
            (WorkflowState::Designing, WorkflowState::ToolCalling { .. }) => true,
            (WorkflowState::Designing, WorkflowState::Escalated) => true,

            // Implementing can go to Reviewing, ToolCalling, or Escalated
            (WorkflowState::Implementing, WorkflowState::Reviewing) => true,
            (WorkflowState::Implementing, WorkflowState::ToolCalling { .. }) => true,
            (WorkflowState::Implementing, WorkflowState::Escalated) => true,

            // Reviewing can go to Implementing (rework), Completed, ToolCalling, or Escalated
            (WorkflowState::Reviewing, WorkflowState::Implementing) => true,
            (WorkflowState::Reviewing, WorkflowState::Completed) => true,
            (WorkflowState::Reviewing, WorkflowState::ToolCalling { .. }) => true,
            (WorkflowState::Reviewing, WorkflowState::Escalated) => true,

            // ToolCalling returns to the state stored in return_to
            (WorkflowState::ToolCalling { return_to }, target) if return_to.as_ref() == target => {
                true
            }

            // Any state can escalate
            (_, WorkflowState::Escalated) => true,

            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_transitions() {
        assert!(WorkflowState::Init.can_transition_to(&WorkflowState::Planning));
        assert!(WorkflowState::Planning.can_transition_to(&WorkflowState::Designing));
        assert!(WorkflowState::Designing.can_transition_to(&WorkflowState::Implementing));
        assert!(WorkflowState::Implementing.can_transition_to(&WorkflowState::Reviewing));
        assert!(WorkflowState::Reviewing.can_transition_to(&WorkflowState::Completed));
        assert!(WorkflowState::Reviewing.can_transition_to(&WorkflowState::Implementing));
    }

    #[test]
    fn invalid_transitions() {
        assert!(!WorkflowState::Init.can_transition_to(&WorkflowState::Completed));
        assert!(!WorkflowState::Init.can_transition_to(&WorkflowState::Implementing));
        assert!(!WorkflowState::Planning.can_transition_to(&WorkflowState::Completed));
        assert!(!WorkflowState::Completed.can_transition_to(&WorkflowState::Init));
    }

    #[test]
    fn tool_calling_returns_to_origin() {
        let tool_state = WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Designing),
        };
        assert!(tool_state.can_transition_to(&WorkflowState::Designing));
        assert!(!tool_state.can_transition_to(&WorkflowState::Implementing));
    }

    #[test]
    fn any_state_can_escalate() {
        let states = vec![
            WorkflowState::Init,
            WorkflowState::Planning,
            WorkflowState::Designing,
            WorkflowState::Implementing,
            WorkflowState::Reviewing,
        ];
        for state in states {
            assert!(
                state.can_transition_to(&WorkflowState::Escalated),
                "{state:?} should be able to escalate"
            );
        }
    }

    #[test]
    fn tool_calling_from_multiple_states() {
        let tc_from_designing = WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Designing),
        };
        let tc_from_implementing = WorkflowState::ToolCalling {
            return_to: Box::new(WorkflowState::Implementing),
        };

        assert!(WorkflowState::Designing.can_transition_to(&tc_from_designing));
        assert!(WorkflowState::Implementing.can_transition_to(&tc_from_implementing));
    }

    #[test]
    fn serialization_roundtrip() {
        let states = vec![
            WorkflowState::Init,
            WorkflowState::ToolCalling {
                return_to: Box::new(WorkflowState::Reviewing),
            },
            WorkflowState::Escalated,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let deserialized: WorkflowState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, deserialized);
        }
    }
}
