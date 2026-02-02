//! Tests for validation module.

use crate::validation::*;
use std::path::Path;

#[test]
fn test_validation_result_valid() {
    let result = ValidationResult::valid();
    assert!(result.valid);
    assert!(result.errors.is_empty());
}

#[test]
fn test_validation_result_invalid() {
    let errors = vec![ValidationError::new("field", "error message")];
    let result = ValidationResult::invalid(errors);
    assert!(!result.valid);
    assert_eq!(result.errors.len(), 1);
}

#[test]
fn test_validation_result_add_error() {
    let mut result = ValidationResult::valid();
    result.add_error(ValidationError::new("test", "error"));

    assert!(!result.valid);
    assert_eq!(result.errors.len(), 1);
}

#[test]
fn test_validation_result_add_warning() {
    let mut result = ValidationResult::valid();
    result.add_warning("this is a warning");

    assert!(result.valid);
    assert_eq!(result.warnings.len(), 1);
}

#[test]
fn test_validation_result_merge() {
    let mut result1 = ValidationResult::valid();
    let mut result2 = ValidationResult::valid();
    result2.add_error(ValidationError::new("field", "error"));

    result1.merge(result2);

    assert!(!result1.valid);
    assert_eq!(result1.errors.len(), 1);
}

#[test]
fn test_validation_error_new() {
    let err = ValidationError::new("field_name", "error message");
    assert_eq!(err.field, "field_name");
    assert_eq!(err.message, "error message");
}

#[test]
fn test_validation_error_with_code() {
    let err = ValidationError::with_code("field", "message", "CUSTOM_CODE");
    assert_eq!(err.code, "CUSTOM_CODE");
}

#[test]
fn test_path_validator_new() {
    let validator = PathValidator::new();
    assert!(validator.allow_relative);
    assert!(!validator.require_exists);
}

#[test]
fn test_path_validator_absolute_only() {
    let validator = PathValidator::new().absolute_only();
    assert!(!validator.allow_relative);
}

#[test]
fn test_path_validator_must_exist() {
    let validator = PathValidator::new().must_exist();
    assert!(validator.require_exists);
}

#[test]
fn test_path_validator_must_be_directory() {
    let validator = PathValidator::new().must_be_directory();
    assert!(validator.require_directory);
}

#[test]
fn test_path_validator_must_be_file() {
    let validator = PathValidator::new().must_be_file();
    assert!(validator.require_file);
}

#[test]
fn test_path_validator_allow_extensions() {
    let validator = PathValidator::new().allow_extensions(vec!["txt", "md"]);
    assert!(validator.allowed_extensions.is_some());
}

#[test]
fn test_path_validator_validate_relative() {
    let validator = PathValidator::new().absolute_only();
    let result = validator.validate(Path::new("relative/path"));
    assert!(!result.valid);
}

#[test]
fn test_path_validator_validate_nonexistent() {
    let validator = PathValidator::new().must_exist();
    let result = validator.validate(Path::new("/nonexistent/path/12345"));
    assert!(!result.valid);
}

#[test]
fn test_command_validator_new() {
    let validator = CommandValidator::new();
    assert!(!validator.blocked.is_empty());
}

#[test]
fn test_command_validator_block() {
    let validator = CommandValidator::new().block("dangerous_cmd");
    assert!(validator.blocked.contains("dangerous_cmd"));
}

#[test]
fn test_command_validator_allow_only() {
    let validator = CommandValidator::new().allow_only(vec!["ls", "pwd"]);
    assert!(validator.allowed.is_some());
}

#[test]
fn test_command_validator_no_shell_operators() {
    let validator = CommandValidator::new().no_shell_operators();
    assert!(!validator.allow_shell_operators);
}

#[test]
fn test_command_validator_validate_safe() {
    let validator = CommandValidator::new();
    let result = validator.validate("ls -la");
    assert!(result.valid);
}

#[test]
fn test_command_validator_validate_blocked() {
    let validator = CommandValidator::new();
    let result = validator.validate("rm -rf /");
    assert!(!result.valid);
}

#[test]
fn test_command_validator_shell_operators() {
    let validator = CommandValidator::new().no_shell_operators();
    let result = validator.validate("echo hello && echo world");
    assert!(!result.valid);
}

#[test]
fn test_url_validator_new() {
    let validator = UrlValidator::new();
    assert!(validator.allowed_schemes.contains(&"http".to_string()));
    assert!(validator.allowed_schemes.contains(&"https".to_string()));
}

#[test]
fn test_url_validator_https_only() {
    let validator = UrlValidator::new().https_only();
    assert!(validator.require_https);
    assert_eq!(validator.allowed_schemes, vec!["https".to_string()]);
}

#[test]
fn test_url_validator_allow_hosts() {
    let validator = UrlValidator::new().allow_hosts(vec!["example.com"]);
    assert!(validator.allowed_hosts.is_some());
}

#[test]
fn test_url_validator_block_host() {
    let validator = UrlValidator::new().block_host("blocked.com");
    assert!(validator.blocked_hosts.contains(&"blocked.com".to_string()));
}

#[test]
fn test_url_validator_validate_valid() {
    let validator = UrlValidator::new();
    let result = validator.validate("https://example.com");
    assert!(result.valid);
}

#[test]
fn test_url_validator_validate_localhost_blocked() {
    let validator = UrlValidator::new();
    let result = validator.validate("http://localhost:8080");
    assert!(!result.valid);
}

#[test]
fn test_url_validator_validate_invalid_url() {
    let validator = UrlValidator::new();
    let result = validator.validate("not a valid url");
    assert!(!result.valid);
}

#[test]
fn test_string_validator_new() {
    let validator = StringValidator::new();
    assert!(validator.min_length.is_none());
    assert!(validator.max_length.is_none());
}

#[test]
fn test_string_validator_min_max() {
    let validator = StringValidator::new().min(3).max(10);
    assert_eq!(validator.min_length, Some(3));
    assert_eq!(validator.max_length, Some(10));
}

#[test]
fn test_string_validator_allowed_chars() {
    let validator = StringValidator::new().allowed_chars("abc123");
    assert!(validator.allowed_chars.is_some());
}

#[test]
fn test_string_validator_validate_too_short() {
    let validator = StringValidator::new().min(5);
    let result = validator.validate("hi");
    assert!(!result.valid);
}

#[test]
fn test_string_validator_validate_too_long() {
    let validator = StringValidator::new().max(5);
    let result = validator.validate("this is too long");
    assert!(!result.valid);
}

#[test]
fn test_string_validator_validate_valid() {
    let validator = StringValidator::new().min(3).max(10);
    let result = validator.validate("hello");
    assert!(result.valid);
}

#[test]
fn test_string_validator_invalid_chars() {
    let validator = StringValidator::new().allowed_chars("abc");
    let result = validator.validate("abcXYZ");
    assert!(!result.valid);
}

#[test]
fn test_number_validator_new() {
    let validator = NumberValidator::<i32>::new();
    assert!(validator.min.is_none());
    assert!(validator.max.is_none());
}

#[test]
fn test_number_validator_min_max() {
    let validator = NumberValidator::<i32>::new().min(0).max(100);
    assert_eq!(validator.min, Some(0));
    assert_eq!(validator.max, Some(100));
}

#[test]
fn test_number_validator_validate_below_min() {
    let validator = NumberValidator::<i32>::new().min(0);
    let result = validator.validate(-1);
    assert!(!result.valid);
}

#[test]
fn test_number_validator_validate_above_max() {
    let validator = NumberValidator::<i32>::new().max(100);
    let result = validator.validate(150);
    assert!(!result.valid);
}

#[test]
fn test_number_validator_validate_valid() {
    let validator = NumberValidator::<i32>::new().min(0).max(100);
    let result = validator.validate(50);
    assert!(result.valid);
}

#[test]
fn test_number_validator_allowed() {
    let validator = NumberValidator::<i32>::new().allowed(vec![1, 2, 3]);
    let result = validator.validate(4);
    assert!(!result.valid);

    let result = validator.validate(2);
    assert!(result.valid);
}

#[test]
fn test_validation_result_to_result_ok() {
    let result = ValidationResult::valid();
    let value = result.to_result("success");
    assert!(value.is_ok());
    assert_eq!(value.unwrap(), "success");
}

#[test]
fn test_validation_result_to_result_err() {
    let mut result = ValidationResult::valid();
    result.add_error(ValidationError::new("field", "error"));

    let value = result.to_result("value");
    assert!(value.is_err());
}

#[test]
fn test_validation_result_default() {
    let result = ValidationResult::default();
    assert!(result.valid);
}

#[test]
fn test_path_validator_default() {
    let validator = PathValidator::default();
    assert!(validator.allow_relative);
}

#[test]
fn test_command_validator_default() {
    let validator = CommandValidator::default();
    assert!(!validator.blocked.is_empty());
}

#[test]
fn test_url_validator_default() {
    let validator = UrlValidator::default();
    assert!(!validator.blocked_hosts.is_empty());
}

#[test]
fn test_string_validator_default() {
    let validator = StringValidator::default();
    assert!(validator.trim);
}

#[test]
fn test_number_validator_default() {
    let validator = NumberValidator::<i32>::default();
    assert!(validator.min.is_none());
}
