use serde::{Deserialize, Serialize};

use super::agent::AgentRole;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub sender: AgentRole,
    pub content: String,
}

impl Message {
    pub fn new(sender: AgentRole, content: impl Into<String>) -> Self {
        Self {
            sender,
            content: content.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_creation() {
        let msg = Message::new(AgentRole::Architect, "Design proposal v1");
        assert_eq!(msg.sender, AgentRole::Architect);
        assert_eq!(msg.content, "Design proposal v1");
    }

    #[test]
    fn message_serialization_roundtrip() {
        let msg = Message::new(AgentRole::Programmer, "impl done");
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, deserialized);
    }
}
