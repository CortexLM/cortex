//! Conversation identifier type.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use schemars::r#gen::SchemaGenerator;
use schemars::schema::{InstanceType, Schema, SchemaObject};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a conversation/session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(Uuid);

impl JsonSchema for ConversationId {
    fn schema_name() -> String {
        "ConversationId".to_string()
    }

    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(InstanceType::String.into()),
            format: Some("uuid".to_string()),
            ..Default::default()
        })
    }
}

impl ConversationId {
    /// Create a new random conversation ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Create from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parse from a string.
    pub fn from_string(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }

    /// Get the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for ConversationId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_string(s)
    }
}

impl From<Uuid> for ConversationId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<ConversationId> for Uuid {
    fn from(id: ConversationId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_id_new() {
        let id1 = ConversationId::new();
        let id2 = ConversationId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_conversation_id_roundtrip() {
        let id = ConversationId::new();
        let s = id.to_string();
        let parsed = ConversationId::from_string(&s).expect("parse");
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_conversation_id_serde() {
        let id = ConversationId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let parsed: ConversationId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, parsed);
    }
}
