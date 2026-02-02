//! Tests for JSON utilities.

use crate::json_utils::*;
use serde_json::json;

#[test]
fn test_json_path_parse_root() {
    let path = JsonPath::parse("$").unwrap();
    let value = json!({"key": "value"});
    let result = path.get(&value);
    assert!(result.is_some());
}

#[test]
fn test_json_path_parse_simple() {
    let path = JsonPath::parse("$.name").unwrap();
    let value = json!({"name": "test"});
    let result = path.get(&value);
    assert_eq!(result, Some(&json!("test")));
}

#[test]
fn test_json_path_parse_nested() {
    let path = JsonPath::parse("$.user.name").unwrap();
    let value = json!({"user": {"name": "John"}});
    let result = path.get(&value);
    assert_eq!(result, Some(&json!("John")));
}

#[test]
fn test_json_path_parse_array_index() {
    let path = JsonPath::parse("$.items[0]").unwrap();
    let value = json!({"items": ["first", "second"]});
    let result = path.get(&value);
    assert_eq!(result, Some(&json!("first")));
}

#[test]
fn test_json_path_parse_empty() {
    let path = JsonPath::parse("").unwrap();
    let value = json!({"key": "value"});
    let result = path.get(&value);
    assert!(result.is_some());
}

#[test]
fn test_json_path_get_missing() {
    let path = JsonPath::parse("$.nonexistent").unwrap();
    let value = json!({"key": "value"});
    let result = path.get(&value);
    assert!(result.is_none());
}

#[test]
fn test_json_path_set_simple() {
    let path = JsonPath::parse("$.name").unwrap();
    let mut value = json!({"name": "old"});
    path.set(&mut value, json!("new")).unwrap();
    assert_eq!(value["name"], "new");
}

#[test]
fn test_json_path_get_mut() {
    let path = JsonPath::parse("$.key").unwrap();
    let mut value = json!({"key": "value"});
    if let Some(v) = path.get_mut(&mut value) {
        *v = json!("modified");
    }
    assert_eq!(value["key"], "modified");
}

#[test]
fn test_parse_json_object() {
    let json = r#"{"key": "value", "number": 42}"#;
    let result: Result<serde_json::Value, _> = serde_json::from_str(json);

    assert!(result.is_ok());
    let value = result.unwrap();
    assert!(value.is_object());
    assert_eq!(value["key"], "value");
    assert_eq!(value["number"], 42);
}

#[test]
fn test_parse_json_array() {
    let json = r#"[1, 2, 3, "four"]"#;
    let result: Result<serde_json::Value, _> = serde_json::from_str(json);

    assert!(result.is_ok());
    let value = result.unwrap();
    assert!(value.is_array());
    assert_eq!(value.as_array().unwrap().len(), 4);
}

#[test]
fn test_parse_invalid_json() {
    let json = r#"{"key": }"#;
    let result: Result<serde_json::Value, _> = serde_json::from_str(json);

    assert!(result.is_err());
}

#[test]
fn test_json_value_types() {
    let null = serde_json::Value::Null;
    let bool_val = serde_json::Value::Bool(true);
    let number = json!(42);
    let string = json!("hello");
    let array = json!([1, 2, 3]);
    let object = json!({"key": "value"});

    assert!(null.is_null());
    assert!(bool_val.is_boolean());
    assert!(number.is_number());
    assert!(string.is_string());
    assert!(array.is_array());
    assert!(object.is_object());
}

#[test]
fn test_json_nested_access() {
    let json = json!({
        "level1": {
            "level2": {
                "level3": "deep value"
            }
        }
    });

    assert_eq!(json["level1"]["level2"]["level3"], "deep value");
}

#[test]
fn test_json_missing_key() {
    let json = json!({"existing": "value"});

    // Accessing missing key returns Null
    assert!(json["missing"].is_null());
}

#[test]
fn test_json_array_indexing() {
    let json = json!([10, 20, 30]);

    assert_eq!(json[0], 10);
    assert_eq!(json[1], 20);
    assert_eq!(json[2], 30);
    assert!(json[99].is_null()); // Out of bounds
}

#[test]
fn test_json_to_string() {
    let json = json!({"key": "value"});
    let string = serde_json::to_string(&json).unwrap();

    assert!(string.contains("key"));
    assert!(string.contains("value"));
}

#[test]
fn test_json_to_string_pretty() {
    let json = json!({"key": "value"});
    let pretty = serde_json::to_string_pretty(&json).unwrap();

    assert!(pretty.contains('\n'));
}

#[test]
fn test_json_merge() {
    let mut base = json!({"a": 1, "b": 2});
    let override_val = json!({"b": 3, "c": 4});

    if let (Some(base_obj), Some(override_obj)) = (base.as_object_mut(), override_val.as_object()) {
        for (k, v) in override_obj {
            base_obj.insert(k.clone(), v.clone());
        }
    }

    assert_eq!(base["a"], 1);
    assert_eq!(base["b"], 3);
    assert_eq!(base["c"], 4);
}

#[test]
fn test_json_number_types() {
    let int = json!(42i64);
    let float = json!(2.5f64);
    let negative = json!(-100);

    assert_eq!(int.as_i64(), Some(42));
    assert_eq!(float.as_f64(), Some(2.5));
    assert_eq!(negative.as_i64(), Some(-100));
}

#[test]
fn test_json_boolean_conversion() {
    let true_val = json!(true);
    let false_val = json!(false);

    assert_eq!(true_val.as_bool(), Some(true));
    assert_eq!(false_val.as_bool(), Some(false));
}

#[test]
fn test_json_string_escaping() {
    let json = json!({"text": "Hello \"World\"\nNew line"});
    let string = serde_json::to_string(&json).unwrap();

    // Should properly escape quotes and newlines
    assert!(string.contains("\\\""));
    assert!(string.contains("\\n"));
}

#[test]
fn test_json_unicode() {
    let json = json!({"greeting": "你好世界 🌍"});
    let string = serde_json::to_string(&json).unwrap();

    // Unicode should be preserved or escaped
    let parsed: serde_json::Value = serde_json::from_str(&string).unwrap();
    assert_eq!(parsed["greeting"], "你好世界 🌍");
}

#[test]
fn test_json_empty_structures() {
    let empty_obj = json!({});
    let empty_arr = json!([]);

    assert!(empty_obj.is_object());
    assert_eq!(empty_obj.as_object().unwrap().len(), 0);

    assert!(empty_arr.is_array());
    assert_eq!(empty_arr.as_array().unwrap().len(), 0);
}

#[test]
fn test_json_large_number() {
    let large = json!(9007199254740992i64);

    assert!(large.is_number());
    assert_eq!(large.as_i64(), Some(9007199254740992i64));
}
