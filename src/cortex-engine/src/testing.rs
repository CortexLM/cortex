//! Testing utilities.
//!
//! Provides utilities for testing including mocking,
//! test fixtures, and assertions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Test result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    /// Test name.
    pub name: String,
    /// Passed.
    pub passed: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Error message if failed.
    pub error: Option<String>,
    /// Output.
    pub output: Option<String>,
    /// Skipped.
    pub skipped: bool,
    /// Skip reason.
    pub skip_reason: Option<String>,
}

impl TestResult {
    /// Create a passing result.
    pub fn pass(name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            passed: true,
            duration_ms,
            error: None,
            output: None,
            skipped: false,
            skip_reason: None,
        }
    }

    /// Create a failing result.
    pub fn fail(name: impl Into<String>, error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            passed: false,
            duration_ms,
            error: Some(error.into()),
            output: None,
            skipped: false,
            skip_reason: None,
        }
    }

    /// Create a skipped result.
    pub fn skip(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: true,
            duration_ms: 0,
            error: None,
            output: None,
            skipped: true,
            skip_reason: Some(reason.into()),
        }
    }

    /// Set output.
    pub fn with_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(output.into());
        self
    }
}

/// Test suite result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestSuiteResult {
    /// Suite name.
    pub name: String,
    /// Test results.
    pub tests: Vec<TestResult>,
    /// Total duration.
    pub duration_ms: u64,
    /// Passed count.
    pub passed: u32,
    /// Failed count.
    pub failed: u32,
    /// Skipped count.
    pub skipped: u32,
}

impl TestSuiteResult {
    /// Create a new suite result.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Add a test result.
    pub fn add(&mut self, result: TestResult) {
        self.duration_ms += result.duration_ms;

        if result.skipped {
            self.skipped += 1;
        } else if result.passed {
            self.passed += 1;
        } else {
            self.failed += 1;
        }

        self.tests.push(result);
    }

    /// Get total count.
    pub fn total(&self) -> u32 {
        self.passed + self.failed + self.skipped
    }

    /// Get pass rate.
    pub fn pass_rate(&self) -> f32 {
        let total = self.passed + self.failed;
        if total == 0 {
            1.0
        } else {
            self.passed as f32 / total as f32
        }
    }

    /// Check if all passed.
    pub fn all_passed(&self) -> bool {
        self.failed == 0
    }

    /// Format summary.
    pub fn summary(&self) -> String {
        format!(
            "{}: {} passed, {} failed, {} skipped ({}ms)",
            self.name, self.passed, self.failed, self.skipped, self.duration_ms
        )
    }
}

/// Mock value.
#[derive(Debug, Clone)]
pub struct MockValue {
    /// The value.
    value: serde_json::Value,
    /// Call count.
    calls: u32,
}

impl MockValue {
    /// Create a new mock value.
    pub fn new<T: Serialize>(value: T) -> Self {
        Self {
            value: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
            calls: 0,
        }
    }

    /// Get the value.
    pub fn get<T: for<'de> Deserialize<'de>>(&mut self) -> Option<T> {
        self.calls += 1;
        serde_json::from_value(self.value.clone()).ok()
    }

    /// Get call count.
    pub fn call_count(&self) -> u32 {
        self.calls
    }
}

/// Mock store.
pub struct MockStore {
    /// Values.
    values: RwLock<HashMap<String, MockValue>>,
    /// Call history.
    history: RwLock<Vec<MockCall>>,
}

impl MockStore {
    /// Create a new store.
    pub fn new() -> Self {
        Self {
            values: RwLock::new(HashMap::new()),
            history: RwLock::new(Vec::new()),
        }
    }

    /// Register a mock value.
    pub async fn register<T: Serialize>(&self, key: impl Into<String>, value: T) {
        self.values
            .write()
            .await
            .insert(key.into(), MockValue::new(value));
    }

    /// Get a mock value.
    pub async fn get<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Option<T> {
        let mut values = self.values.write().await;
        values.get_mut(key).and_then(MockValue::get)
    }

    /// Record a call.
    pub async fn record_call(&self, name: impl Into<String>, args: Vec<serde_json::Value>) {
        self.history.write().await.push(MockCall {
            name: name.into(),
            args,
            timestamp: timestamp_now(),
        });
    }

    /// Get call history.
    pub async fn call_history(&self, name: &str) -> Vec<MockCall> {
        self.history
            .read()
            .await
            .iter()
            .filter(|c| c.name == name)
            .cloned()
            .collect()
    }

    /// Get call count.
    pub async fn call_count(&self, name: &str) -> usize {
        self.history
            .read()
            .await
            .iter()
            .filter(|c| c.name == name)
            .count()
    }

    /// Clear history.
    pub async fn clear(&self) {
        self.values.write().await.clear();
        self.history.write().await.clear();
    }
}

impl Default for MockStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Mock call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockCall {
    /// Function name.
    pub name: String,
    /// Arguments.
    pub args: Vec<serde_json::Value>,
    /// Timestamp.
    pub timestamp: u64,
}

/// Test fixture.
pub struct TestFixture {
    /// Temp directory.
    temp_dir: Option<tempfile::TempDir>,
    /// Mock store.
    mocks: MockStore,
    /// Environment overrides.
    env_overrides: HashMap<String, String>,
    /// Original env values.
    original_env: HashMap<String, Option<String>>,
}

impl TestFixture {
    /// Create a new fixture.
    pub fn new() -> Self {
        Self {
            temp_dir: None,
            mocks: MockStore::new(),
            env_overrides: HashMap::new(),
            original_env: HashMap::new(),
        }
    }

    /// Create with temp directory.
    pub fn with_temp_dir() -> std::io::Result<Self> {
        let temp = tempfile::tempdir()?;
        Ok(Self {
            temp_dir: Some(temp),
            mocks: MockStore::new(),
            env_overrides: HashMap::new(),
            original_env: HashMap::new(),
        })
    }

    /// Get temp directory path.
    pub fn temp_path(&self) -> Option<&Path> {
        self.temp_dir.as_ref().map(tempfile::TempDir::path)
    }

    /// Create a temp file.
    pub fn create_file(&self, name: &str, content: &str) -> std::io::Result<PathBuf> {
        let path = self
            .temp_path()
            .ok_or_else(|| std::io::Error::other("No temp directory"))?
            .join(name);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// Get mock store.
    pub fn mocks(&self) -> &MockStore {
        &self.mocks
    }

    /// Set environment variable.
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();

        // Save original
        if !self.original_env.contains_key(&key) {
            self.original_env
                .insert(key.clone(), std::env::var(&key).ok());
        }

        // SAFETY: We restore the original values in Drop
        unsafe {
            std::env::set_var(&key, &value);
        }
        self.env_overrides.insert(key, value);
    }

    /// Unset environment variable.
    pub fn unset_env(&mut self, key: impl Into<String>) {
        let key = key.into();

        // Save original
        if !self.original_env.contains_key(&key) {
            self.original_env
                .insert(key.clone(), std::env::var(&key).ok());
        }

        // SAFETY: We restore the original values in Drop
        unsafe {
            std::env::remove_var(&key);
        }
    }
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        // Restore environment
        // SAFETY: Restoring original environment values
        unsafe {
            for (key, original) in &self.original_env {
                match original {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

impl Default for TestFixture {
    fn default() -> Self {
        Self::new()
    }
}

/// Assertion helpers.
pub mod assert {
    use super::*;

    /// Assert that a value is Some.
    pub fn is_some<T>(option: &Option<T>, message: &str) {
        if option.is_none() {
            panic!("Expected Some, got None: {message}");
        }
    }

    /// Assert that a value is None.
    pub fn is_none<T>(option: &Option<T>, message: &str) {
        if option.is_some() {
            panic!("Expected None, got Some: {message}");
        }
    }

    /// Assert that a result is Ok.
    pub fn is_ok<T, E: std::fmt::Debug>(result: &Result<T, E>, message: &str) {
        if let Err(e) = result {
            panic!("Expected Ok, got Err({e:?}): {message}");
        }
    }

    /// Assert that a result is Err.
    pub fn is_err<T: std::fmt::Debug, E>(result: &Result<T, E>, message: &str) {
        if let Ok(v) = result {
            panic!("Expected Err, got Ok({v:?}): {message}");
        }
    }

    /// Assert that a string contains a substring.
    pub fn contains(haystack: &str, needle: &str, message: &str) {
        if !haystack.contains(needle) {
            panic!("Expected '{haystack}' to contain '{needle}': {message}");
        }
    }

    /// Assert that a string does not contain a substring.
    pub fn not_contains(haystack: &str, needle: &str, message: &str) {
        if haystack.contains(needle) {
            panic!("Expected '{haystack}' to not contain '{needle}': {message}");
        }
    }

    /// Assert that a collection is empty.
    pub fn is_empty<T>(collection: &[T], message: &str) {
        if !collection.is_empty() {
            panic!(
                "Expected empty collection, got {} items: {}",
                collection.len(),
                message
            );
        }
    }

    /// Assert that a collection is not empty.
    pub fn not_empty<T>(collection: &[T], message: &str) {
        if collection.is_empty() {
            panic!("Expected non-empty collection: {message}");
        }
    }

    /// Assert that a path exists.
    pub fn path_exists(path: &Path, message: &str) {
        if !path.exists() {
            panic!("Expected path to exist: {path:?}: {message}");
        }
    }

    /// Assert that a path does not exist.
    pub fn path_not_exists(path: &Path, message: &str) {
        if path.exists() {
            panic!("Expected path to not exist: {path:?}: {message}");
        }
    }

    /// Assert approximate equality for floats.
    pub fn approx_eq(a: f64, b: f64, epsilon: f64, message: &str) {
        if (a - b).abs() > epsilon {
            panic!("Expected {a} ≈ {b} (ε={epsilon}): {message}");
        }
    }
}

/// Test runner.
pub struct TestRunner {
    /// Test cases.
    cases: Vec<TestCase>,
    /// Setup function.
    setup: Option<Box<dyn Fn() -> TestFixture + Send + Sync>>,
}

impl TestRunner {
    /// Create a new runner.
    pub fn new() -> Self {
        Self {
            cases: Vec::new(),
            setup: None,
        }
    }

    /// Set setup function.
    pub fn setup<F>(mut self, f: F) -> Self
    where
        F: Fn() -> TestFixture + Send + Sync + 'static,
    {
        self.setup = Some(Box::new(f));
        self
    }

    /// Add a test case.
    pub fn test<F>(mut self, name: impl Into<String>, f: F) -> Self
    where
        F: Fn(&TestFixture) -> Result<(), String> + Send + Sync + 'static,
    {
        self.cases.push(TestCase {
            name: name.into(),
            test_fn: Box::new(f),
            skip: false,
            skip_reason: None,
        });
        self
    }

    /// Run all tests.
    pub fn run(&self) -> TestSuiteResult {
        let mut results = TestSuiteResult::new("Test Suite");

        for case in &self.cases {
            let fixture = if let Some(ref setup) = self.setup {
                setup()
            } else {
                TestFixture::new()
            };

            if case.skip {
                results.add(TestResult::skip(
                    &case.name,
                    case.skip_reason.as_deref().unwrap_or("Skipped"),
                ));
                continue;
            }

            let start = std::time::Instant::now();
            let result = (case.test_fn)(&fixture);
            let duration = start.elapsed().as_millis() as u64;

            match result {
                Ok(()) => results.add(TestResult::pass(&case.name, duration)),
                Err(e) => results.add(TestResult::fail(&case.name, e, duration)),
            }
        }

        results
    }
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Test case.
struct TestCase {
    name: String,
    test_fn: Box<dyn Fn(&TestFixture) -> Result<(), String> + Send + Sync>,
    skip: bool,
    skip_reason: Option<String>,
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_test_result() {
        let pass = TestResult::pass("test1", 100);
        assert!(pass.passed);
        assert!(!pass.skipped);

        let fail = TestResult::fail("test2", "error", 50);
        assert!(!fail.passed);
        assert_eq!(fail.error, Some("error".to_string()));

        let skip = TestResult::skip("test3", "not implemented");
        assert!(skip.skipped);
    }

    #[test]
    fn test_suite_result() {
        let mut suite = TestSuiteResult::new("suite");

        suite.add(TestResult::pass("test1", 100));
        suite.add(TestResult::pass("test2", 50));
        suite.add(TestResult::fail("test3", "err", 25));

        assert_eq!(suite.passed, 2);
        assert_eq!(suite.failed, 1);
        assert_eq!(suite.total(), 3);
        assert!(!suite.all_passed());
    }

    #[tokio::test]
    async fn test_mock_store() {
        let store = MockStore::new();

        store.register("key1", "value1").await;

        let value: Option<String> = store.get("key1").await;
        assert_eq!(value, Some("value1".to_string()));

        store
            .record_call("func1", vec![serde_json::json!(42)])
            .await;
        assert_eq!(store.call_count("func1").await, 1);
    }

    #[test]
    fn test_fixture() {
        let mut fixture = TestFixture::new();

        fixture.set_env("TEST_VAR_123", "test_value");
        assert_eq!(
            std::env::var("TEST_VAR_123").ok(),
            Some("test_value".to_string())
        );

        drop(fixture);
        // Env should be restored
    }

    #[test]
    fn test_fixture_with_temp() {
        let fixture = TestFixture::with_temp_dir().unwrap();
        let path = fixture.create_file("test.txt", "content").unwrap();

        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
    }

    #[test]
    fn test_assertions() {
        assert::is_some(&Some(42), "should be some");
        assert::is_none::<i32>(&None, "should be none");
        assert::contains("hello world", "world", "should contain");
        assert::not_empty(&[1, 2, 3], "should not be empty");
        assert::approx_eq(1.0, 1.001, 0.01, "should be approximately equal");
    }

    #[test]
    fn test_runner() {
        let results = TestRunner::new()
            .test("passing", |_| Ok(()))
            .test("failing", |_| Err("intentional".to_string()))
            .run();

        assert_eq!(results.passed, 1);
        assert_eq!(results.failed, 1);
    }
}
