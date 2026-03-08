use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentRole {
    Orchestrator,
    Architect,
    Programmer,
    DevilsAdvocate,
}

impl AgentRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentRole::Orchestrator => "Orchestrator",
            AgentRole::Architect => "Architect",
            AgentRole::Programmer => "Programmer",
            AgentRole::DevilsAdvocate => "DevilsAdvocate",
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_role_serialization_roundtrip() {
        for role in [
            AgentRole::Orchestrator,
            AgentRole::Architect,
            AgentRole::Programmer,
            AgentRole::DevilsAdvocate,
        ] {
            let json = serde_json::to_string(&role).unwrap();
            let deserialized: AgentRole = serde_json::from_str(&json).unwrap();
            assert_eq!(role, deserialized);
        }
    }

    #[test]
    fn agent_role_display() {
        assert_eq!(AgentRole::Orchestrator.to_string(), "Orchestrator");
        assert_eq!(AgentRole::DevilsAdvocate.to_string(), "DevilsAdvocate");
    }
}
