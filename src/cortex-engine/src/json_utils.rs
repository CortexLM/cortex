//! JSON utilities.
//!
//! Provides utilities for working with JSON including
//! path-based access, transformation, and validation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{CortexError, Result};

/// JSON path for navigating JSON structures.
#[derive(Debug, Clone)]
pub struct JsonPath {
    segments: Vec<PathSegment>,
}

impl JsonPath {
    /// Parse a path string.
    pub fn parse(path: &str) -> Result<Self> {
        let mut segments = Vec::new();

        if path.is_empty() || path == "$" {
            return Ok(Self { segments });
        }

        let path = path.strip_prefix("$.").unwrap_or(path);

        for part in path.split('.') {
            if part.is_empty() {
                continue;
            }

            // Check for array index
            if part.contains('[') && part.ends_with(']') {
                let bracket_pos = part.find('[').unwrap();
                let key = &part[..bracket_pos];
                let index_str = &part[bracket_pos + 1..part.len() - 1];

                if !key.is_empty() {
                    segments.push(PathSegment::Key(key.to_string()));
                }

                let index: usize = index_str.parse().map_err(|_| {
                    CortexError::InvalidInput(format!("Invalid array index: {index_str}"))
                })?;
                segments.push(PathSegment::Index(index));
            } else {
                segments.push(PathSegment::Key(part.to_string()));
            }
        }

        Ok(Self { segments })
    }

    /// Get value at path.
    pub fn get<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        let mut current = value;

        for segment in &self.segments {
            match segment {
                PathSegment::Key(key) => {
                    current = current.get(key)?;
                }
                PathSegment::Index(idx) => {
                    current = current.get(idx)?;
                }
            }
        }

        Some(current)
    }

    /// Get mutable value at path.
    pub fn get_mut<'a>(&self, value: &'a mut Value) -> Option<&'a mut Value> {
        let mut current = value;

        for segment in &self.segments {
            match segment {
                PathSegment::Key(key) => {
                    current = current.get_mut(key)?;
                }
                PathSegment::Index(idx) => {
                    current = current.get_mut(idx)?;
                }
            }
        }

        Some(current)
    }

    /// Set value at path.
    pub fn set(&self, root: &mut Value, new_value: Value) -> Result<()> {
        if self.segments.is_empty() {
            *root = new_value;
            return Ok(());
        }

        let parent_path = Self {
            segments: self.segments[..self.segments.len() - 1].to_vec(),
        };

        let parent = parent_path
            .get_mut(root)
            .ok_or_else(|| CortexError::NotFound("Parent path not found".to_string()))?;

        match &self.segments[self.segments.len() - 1] {
            PathSegment::Key(key) => {
                if let Some(obj) = parent.as_object_mut() {
                    obj.insert(key.clone(), new_value);
                } else {
                    return Err(CortexError::InvalidInput(
                        "Parent is not an object".to_string(),
                    ));
                }
            }
            PathSegment::Index(idx) => {
                if let Some(arr) = parent.as_array_mut() {
                    if *idx < arr.len() {
                        arr[*idx] = new_value;
                    } else {
                        return Err(CortexError::InvalidInput("Index out of bounds".to_string()));
                    }
                } else {
                    return Err(CortexError::InvalidInput(
                        "Parent is not an array".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Delete value at path.
    pub fn delete(&self, root: &mut Value) -> Result<Option<Value>> {
        if self.segments.is_empty() {
            return Err(CortexError::InvalidInput("Cannot delete root".to_string()));
        }

        let parent_path = Self {
            segments: self.segments[..self.segments.len() - 1].to_vec(),
        };

        let parent = parent_path
            .get_mut(root)
            .ok_or_else(|| CortexError::NotFound("Parent path not found".to_string()))?;

        match &self.segments[self.segments.len() - 1] {
            PathSegment::Key(key) => {
                if let Some(obj) = parent.as_object_mut() {
                    Ok(obj.remove(key))
                } else {
                    Err(CortexError::InvalidInput(
                        "Parent is not an object".to_string(),
                    ))
                }
            }
            PathSegment::Index(idx) => {
                if let Some(arr) = parent.as_array_mut() {
                    if *idx < arr.len() {
                        Ok(Some(arr.remove(*idx)))
                    } else {
                        Err(CortexError::InvalidInput("Index out of bounds".to_string()))
                    }
                } else {
                    Err(CortexError::InvalidInput(
                        "Parent is not an array".to_string(),
                    ))
                }
            }
        }
    }

    /// Check if path exists.
    pub fn exists(&self, value: &Value) -> bool {
        self.get(value).is_some()
    }

    /// Convert to string.
    pub fn to_string(&self) -> String {
        let mut parts = vec!["$".to_string()];
        for segment in &self.segments {
            match segment {
                PathSegment::Key(key) => parts.push(key.clone()),
                PathSegment::Index(idx) => parts.push(format!("[{idx}]")),
            }
        }
        parts.join(".")
    }
}

/// Path segment.
#[derive(Debug, Clone)]
enum PathSegment {
    Key(String),
    Index(usize),
}

/// JSON merge.
pub fn merge(base: &mut Value, other: &Value) {
    match (base, other) {
        (Value::Object(base_obj), Value::Object(other_obj)) => {
            for (key, value) in other_obj {
                if let Some(base_value) = base_obj.get_mut(key) {
                    merge(base_value, value);
                } else {
                    base_obj.insert(key.clone(), value.clone());
                }
            }
        }
        (base, other) => {
            *base = other.clone();
        }
    }
}

/// JSON diff.
pub fn diff(old: &Value, new: &Value) -> Vec<JsonDiff> {
    let mut diffs = Vec::new();
    diff_recursive(old, new, String::new(), &mut diffs);
    diffs
}

fn diff_recursive(old: &Value, new: &Value, path: String, diffs: &mut Vec<JsonDiff>) {
    if old == new {
        return;
    }

    match (old, new) {
        (Value::Object(old_obj), Value::Object(new_obj)) => {
            // Check for removed keys
            for key in old_obj.keys() {
                if !new_obj.contains_key(key) {
                    let key_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    diffs.push(JsonDiff::Removed {
                        path: key_path,
                        value: old_obj.get(key).unwrap().clone(),
                    });
                }
            }

            // Check for added and changed keys
            for (key, new_value) in new_obj {
                let key_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };

                if let Some(old_value) = old_obj.get(key) {
                    diff_recursive(old_value, new_value, key_path, diffs);
                } else {
                    diffs.push(JsonDiff::Added {
                        path: key_path,
                        value: new_value.clone(),
                    });
                }
            }
        }
        (Value::Array(old_arr), Value::Array(new_arr)) => {
            let max_len = old_arr.len().max(new_arr.len());
            for i in 0..max_len {
                let idx_path = if path.is_empty() {
                    format!("[{i}]")
                } else {
                    format!("{path}[{i}]")
                };

                match (old_arr.get(i), new_arr.get(i)) {
                    (Some(old_val), Some(new_val)) => {
                        diff_recursive(old_val, new_val, idx_path, diffs);
                    }
                    (Some(old_val), None) => {
                        diffs.push(JsonDiff::Removed {
                            path: idx_path,
                            value: old_val.clone(),
                        });
                    }
                    (None, Some(new_val)) => {
                        diffs.push(JsonDiff::Added {
                            path: idx_path,
                            value: new_val.clone(),
                        });
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            diffs.push(JsonDiff::Changed {
                path,
                old_value: old.clone(),
                new_value: new.clone(),
            });
        }
    }
}

/// JSON diff entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum JsonDiff {
    /// Value was added.
    Added { path: String, value: Value },
    /// Value was removed.
    Removed { path: String, value: Value },
    /// Value was changed.
    Changed {
        path: String,
        old_value: Value,
        new_value: Value,
    },
}

impl JsonDiff {
    /// Get path.
    pub fn path(&self) -> &str {
        match self {
            Self::Added { path, .. } => path,
            Self::Removed { path, .. } => path,
            Self::Changed { path, .. } => path,
        }
    }
}

/// JSON flattener.
pub fn flatten(value: &Value) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    flatten_recursive(value, String::new(), &mut result);
    result
}

fn flatten_recursive(value: &Value, prefix: String, result: &mut HashMap<String, Value>) {
    match value {
        Value::Object(obj) => {
            for (key, val) in obj {
                let new_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_recursive(val, new_prefix, result);
            }
        }
        Value::Array(arr) => {
            for (idx, val) in arr.iter().enumerate() {
                let new_prefix = format!("{prefix}[{idx}]");
                flatten_recursive(val, new_prefix, result);
            }
        }
        _ => {
            result.insert(prefix, value.clone());
        }
    }
}

/// JSON unflatten.
pub fn unflatten(flat: &HashMap<String, Value>) -> Value {
    let mut result = Value::Object(Map::new());

    for (path, value) in flat {
        let json_path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });

        // Create path
        let _ = ensure_path(&mut result, &json_path);
        let _ = json_path.set(&mut result, value.clone());
    }

    result
}

/// Ensure path exists in value.
fn ensure_path(root: &mut Value, path: &JsonPath) -> Result<()> {
    let mut current = root;

    for (i, segment) in path.segments.iter().enumerate() {
        match segment {
            PathSegment::Key(key) => {
                if !current.is_object() {
                    *current = Value::Object(Map::new());
                }
                let obj = current.as_object_mut().unwrap();
                if !obj.contains_key(key) {
                    // Look ahead to determine type
                    let next_is_index = path
                        .segments
                        .get(i + 1)
                        .map(|s| matches!(s, PathSegment::Index(_)))
                        .unwrap_or(false);

                    if next_is_index {
                        obj.insert(key.clone(), Value::Array(vec![]));
                    } else {
                        obj.insert(key.clone(), Value::Object(Map::new()));
                    }
                }
                current = obj.get_mut(key).unwrap();
            }
            PathSegment::Index(idx) => {
                if !current.is_array() {
                    *current = Value::Array(vec![]);
                }
                let arr = current.as_array_mut().unwrap();
                while arr.len() <= *idx {
                    arr.push(Value::Null);
                }
                current = &mut arr[*idx];
            }
        }
    }

    Ok(())
}

/// JSON query builder.
pub struct JsonQuery {
    filters: Vec<Box<dyn Fn(&Value) -> bool + Send + Sync>>,
}

impl JsonQuery {
    /// Create a new query.
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
        }
    }

    /// Add equals filter.
    pub fn equals(mut self, path: &str, expected: Value) -> Self {
        let path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });
        self.filters.push(Box::new(move |v| {
            path.get(v).map(|v| v == &expected).unwrap_or(false)
        }));
        self
    }

    /// Add contains filter (for strings/arrays).
    pub fn contains(mut self, path: &str, needle: &str) -> Self {
        let path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });
        let needle = needle.to_string();
        self.filters.push(Box::new(move |v| {
            if let Some(val) = path.get(v)
                && let Some(s) = val.as_str()
            {
                return s.contains(&needle);
            }
            false
        }));
        self
    }

    /// Add exists filter.
    pub fn exists(mut self, path: &str) -> Self {
        let path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });
        self.filters.push(Box::new(move |v| path.exists(v)));
        self
    }

    /// Add greater than filter.
    pub fn gt(mut self, path: &str, threshold: f64) -> Self {
        let path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });
        self.filters.push(Box::new(move |v| {
            path.get(v)
                .and_then(serde_json::Value::as_f64)
                .map(|n| n > threshold)
                .unwrap_or(false)
        }));
        self
    }

    /// Add less than filter.
    pub fn lt(mut self, path: &str, threshold: f64) -> Self {
        let path = JsonPath::parse(path).unwrap_or(JsonPath { segments: vec![] });
        self.filters.push(Box::new(move |v| {
            path.get(v)
                .and_then(serde_json::Value::as_f64)
                .map(|n| n < threshold)
                .unwrap_or(false)
        }));
        self
    }

    /// Check if value matches query.
    pub fn matches(&self, value: &Value) -> bool {
        self.filters.iter().all(|f| f(value))
    }

    /// Filter array of values.
    pub fn filter<'a>(&self, values: &'a [Value]) -> Vec<&'a Value> {
        values.iter().filter(|v| self.matches(v)).collect()
    }
}

impl Default for JsonQuery {
    fn default() -> Self {
        Self::new()
    }
}

/// Pretty print JSON.
pub fn pretty_print(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Compact JSON.
pub fn compact(value: &Value) -> String {
    value.to_string()
}

/// Validate JSON against schema (simplified).
pub fn validate_type(value: &Value, expected_type: JsonType) -> bool {
    match expected_type {
        JsonType::Null => value.is_null(),
        JsonType::Bool => value.is_boolean(),
        JsonType::Number => value.is_number(),
        JsonType::String => value.is_string(),
        JsonType::Array => value.is_array(),
        JsonType::Object => value.is_object(),
    }
}

/// JSON type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonType {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_path_parse() {
        let path = JsonPath::parse("user.name").unwrap();
        assert_eq!(path.segments.len(), 2);

        let path = JsonPath::parse("items[0].value").unwrap();
        assert_eq!(path.segments.len(), 3);
    }

    #[test]
    fn test_json_path_get() {
        let data = json!({
            "user": {
                "name": "Alice",
                "age": 30
            },
            "items": [1, 2, 3]
        });

        let path = JsonPath::parse("user.name").unwrap();
        assert_eq!(path.get(&data), Some(&json!("Alice")));

        let path = JsonPath::parse("items[1]").unwrap();
        assert_eq!(path.get(&data), Some(&json!(2)));
    }

    #[test]
    fn test_json_path_set() {
        let mut data = json!({
            "user": { "name": "Alice" }
        });

        let path = JsonPath::parse("user.name").unwrap();
        path.set(&mut data, json!("Bob")).unwrap();

        assert_eq!(data["user"]["name"], json!("Bob"));
    }

    #[test]
    fn test_json_merge() {
        let mut base = json!({
            "a": 1,
            "b": { "c": 2 }
        });

        let other = json!({
            "b": { "d": 3 },
            "e": 4
        });

        merge(&mut base, &other);

        assert_eq!(base["a"], json!(1));
        assert_eq!(base["b"]["c"], json!(2));
        assert_eq!(base["b"]["d"], json!(3));
        assert_eq!(base["e"], json!(4));
    }

    #[test]
    fn test_json_diff() {
        let old = json!({ "a": 1, "b": 2 });
        let new = json!({ "a": 1, "c": 3 });

        let diffs = diff(&old, &new);

        assert!(
            diffs
                .iter()
                .any(|d| matches!(d, JsonDiff::Removed { path, .. } if path == "b"))
        );
        assert!(
            diffs
                .iter()
                .any(|d| matches!(d, JsonDiff::Added { path, .. } if path == "c"))
        );
    }

    #[test]
    fn test_json_flatten() {
        let data = json!({
            "user": {
                "name": "Alice"
            },
            "items": [1, 2]
        });

        let flat = flatten(&data);

        assert_eq!(flat.get("user.name"), Some(&json!("Alice")));
        assert_eq!(flat.get("items[0]"), Some(&json!(1)));
    }

    #[test]
    fn test_json_query() {
        let data = vec![
            json!({ "name": "Alice", "age": 30 }),
            json!({ "name": "Bob", "age": 25 }),
            json!({ "name": "Charlie", "age": 35 }),
        ];

        let query = JsonQuery::new().gt("age", 28.0);

        let results = query.filter(&data);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_validate_type() {
        assert!(validate_type(&json!("test"), JsonType::String));
        assert!(validate_type(&json!(42), JsonType::Number));
        assert!(validate_type(&json!([1, 2]), JsonType::Array));
        assert!(!validate_type(&json!("test"), JsonType::Number));
    }
}
