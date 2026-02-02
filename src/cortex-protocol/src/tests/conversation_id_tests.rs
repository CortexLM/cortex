//! Comprehensive tests for ConversationId.

use crate::conversation_id::ConversationId;
use std::collections::HashSet;

#[test]
fn test_conversation_id_new_uniqueness() {
    let mut ids = HashSet::new();

    for _ in 0..100 {
        let id = ConversationId::new();
        assert!(ids.insert(id), "Generated duplicate ID");
    }
}

#[test]
fn test_conversation_id_default() {
    let id1 = ConversationId::default();
    let id2 = ConversationId::default();

    // Default creates new unique IDs
    assert_ne!(id1, id2);
}

#[test]
fn test_conversation_id_from_uuid() {
    let uuid = uuid::Uuid::new_v4();
    let id = ConversationId::from_uuid(uuid);

    assert_eq!(*id.as_uuid(), uuid);
}

#[test]
fn test_conversation_id_from_string_valid() {
    let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
    let id = ConversationId::from_string(uuid_str).expect("valid UUID");

    assert_eq!(id.to_string(), uuid_str);
}

#[test]
fn test_conversation_id_from_string_invalid() {
    let invalid_strings = vec![
        "",
        "not-a-uuid",
        "550e8400-e29b-41d4-a716",
        "550e8400-e29b-41d4-a716-446655440000-extra",
        "gggggggg-gggg-gggg-gggg-gggggggggggg",
    ];

    for s in invalid_strings {
        assert!(
            ConversationId::from_string(s).is_err(),
            "Should fail for: {}",
            s
        );
    }
}

#[test]
fn test_conversation_id_from_str_trait() {
    let uuid_str = "a0b1c2d3-e4f5-6789-abcd-ef0123456789";
    let id: ConversationId = uuid_str.parse().expect("parse");

    assert_eq!(id.to_string(), uuid_str);
}

#[test]
fn test_conversation_id_display() {
    let uuid_str = "12345678-1234-5678-1234-567812345678";
    let id = ConversationId::from_string(uuid_str).expect("valid");

    assert_eq!(format!("{}", id), uuid_str);
}

#[test]
fn test_conversation_id_into_uuid() {
    let original_uuid = uuid::Uuid::new_v4();
    let id = ConversationId::from_uuid(original_uuid);

    let uuid_back: uuid::Uuid = id.into();
    assert_eq!(uuid_back, original_uuid);
}

#[test]
fn test_conversation_id_from_uuid_trait() {
    let uuid = uuid::Uuid::new_v4();
    let id: ConversationId = uuid.into();

    assert_eq!(*id.as_uuid(), uuid);
}

#[test]
fn test_conversation_id_equality() {
    let uuid = uuid::Uuid::new_v4();
    let id1 = ConversationId::from_uuid(uuid);
    let id2 = ConversationId::from_uuid(uuid);
    let id3 = ConversationId::new();

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
}

#[test]
fn test_conversation_id_hash() {
    let uuid = uuid::Uuid::new_v4();
    let id1 = ConversationId::from_uuid(uuid);
    let id2 = ConversationId::from_uuid(uuid);

    let mut set = HashSet::new();
    set.insert(id1);

    assert!(set.contains(&id2));
}

#[test]
fn test_conversation_id_serde_json() {
    let id = ConversationId::new();

    let json = serde_json::to_string(&id).expect("serialize");
    let parsed: ConversationId = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(id, parsed);
}

#[test]
fn test_conversation_id_serde_json_format() {
    let uuid_str = "11111111-2222-3333-4444-555555555555";
    let id = ConversationId::from_string(uuid_str).expect("valid");

    let json = serde_json::to_string(&id).expect("serialize");

    // Should serialize as a plain string (transparent)
    assert_eq!(json, format!("\"{}\"", uuid_str));
}

#[test]
fn test_conversation_id_debug() {
    let id = ConversationId::new();
    let debug = format!("{:?}", id);

    assert!(debug.contains("ConversationId"));
}

#[test]
fn test_conversation_id_clone() {
    let id = ConversationId::new();
    let cloned = id;

    assert_eq!(id, cloned);
}

#[test]
fn test_conversation_id_copy() {
    let id = ConversationId::new();
    let copied = id; // Copy trait

    assert_eq!(id, copied);
}

#[test]
fn test_conversation_id_roundtrip_all_variants() {
    // Test various UUID versions/variants
    let uuids = vec![
        "00000000-0000-0000-0000-000000000000", // Nil UUID
        "ffffffff-ffff-ffff-ffff-ffffffffffff", // Max UUID
        "550e8400-e29b-41d4-a716-446655440000", // Standard v4
    ];

    for uuid_str in uuids {
        let id = ConversationId::from_string(uuid_str).expect("valid");
        let roundtrip = id.to_string();
        assert_eq!(uuid_str, roundtrip);
    }
}

#[test]
fn test_conversation_id_case_sensitivity() {
    // UUIDs should be parsed case-insensitively
    let lower = "abcdef12-3456-7890-abcd-ef1234567890";
    let upper = "ABCDEF12-3456-7890-ABCD-EF1234567890";

    let id_lower = ConversationId::from_string(lower).expect("valid");
    let id_upper = ConversationId::from_string(upper).expect("valid");

    assert_eq!(id_lower, id_upper);
}

#[test]
fn test_conversation_id_in_struct() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        id: ConversationId,
        name: String,
    }

    let s = TestStruct {
        id: ConversationId::new(),
        name: "test".to_string(),
    };

    let json = serde_json::to_string(&s).expect("serialize");
    let parsed: TestStruct = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(s.id, parsed.id);
    assert_eq!(s.name, parsed.name);
}
