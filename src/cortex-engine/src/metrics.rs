//! Metrics and telemetry collection.
//!
//! Provides comprehensive metrics collection, aggregation, and reporting
//! for monitoring agent performance and behavior.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Metrics configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Enable metrics collection.
    pub enabled: bool,
    /// Collection interval.
    pub interval: Duration,
    /// Maximum history size.
    pub max_history: usize,
    /// Enable detailed timing.
    pub detailed_timing: bool,
    /// Export format.
    pub export_format: ExportFormat,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: Duration::from_secs(60),
            max_history: 1000,
            detailed_timing: true,
            export_format: ExportFormat::Json,
        }
    }
}

/// Export format for metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// JSON format.
    Json,
    /// Prometheus format.
    Prometheus,
    /// OpenTelemetry format.
    OpenTelemetry,
}

/// Main metrics collector.
#[allow(dead_code)]
pub struct MetricsCollector {
    /// Configuration.
    config: MetricsConfig,
    /// Counters.
    counters: RwLock<HashMap<String, Counter>>,
    /// Gauges.
    gauges: RwLock<HashMap<String, Gauge>>,
    /// Histograms.
    histograms: RwLock<HashMap<String, Histogram>>,
    /// Timers.
    timers: RwLock<HashMap<String, Timer>>,
    /// Start time.
    start_time: Instant,
    /// Labels.
    labels: RwLock<HashMap<String, String>>,
}

impl MetricsCollector {
    /// Create a new metrics collector.
    pub fn new(config: MetricsConfig) -> Self {
        Self {
            config,
            counters: RwLock::new(HashMap::new()),
            gauges: RwLock::new(HashMap::new()),
            histograms: RwLock::new(HashMap::new()),
            timers: RwLock::new(HashMap::new()),
            start_time: Instant::now(),
            labels: RwLock::new(HashMap::new()),
        }
    }

    /// Create with default config.
    pub fn default_collector() -> Self {
        Self::new(MetricsConfig::default())
    }

    /// Add a global label.
    pub async fn add_label(&self, key: impl Into<String>, value: impl Into<String>) {
        self.labels.write().await.insert(key.into(), value.into());
    }

    // Counter operations

    /// Increment a counter.
    pub async fn increment(&self, name: &str) {
        self.increment_by(name, 1).await;
    }

    /// Increment a counter by a value.
    pub async fn increment_by(&self, name: &str, value: u64) {
        let mut counters = self.counters.write().await;
        counters
            .entry(name.to_string())
            .or_insert_with(Counter::new)
            .add(value);
    }

    /// Get counter value.
    pub async fn counter_value(&self, name: &str) -> u64 {
        self.counters
            .read()
            .await
            .get(name)
            .map(Counter::value)
            .unwrap_or(0)
    }

    // Gauge operations

    /// Set a gauge value.
    pub async fn gauge_set(&self, name: &str, value: f64) {
        let mut gauges = self.gauges.write().await;
        gauges
            .entry(name.to_string())
            .or_insert_with(Gauge::new)
            .set(value);
    }

    /// Increment a gauge.
    pub async fn gauge_inc(&self, name: &str) {
        self.gauge_add(name, 1.0).await;
    }

    /// Decrement a gauge.
    pub async fn gauge_dec(&self, name: &str) {
        self.gauge_add(name, -1.0).await;
    }

    /// Add to a gauge.
    pub async fn gauge_add(&self, name: &str, value: f64) {
        let mut gauges = self.gauges.write().await;
        gauges
            .entry(name.to_string())
            .or_insert_with(Gauge::new)
            .add(value);
    }

    /// Get gauge value.
    pub async fn gauge_value(&self, name: &str) -> f64 {
        self.gauges
            .read()
            .await
            .get(name)
            .map(Gauge::value)
            .unwrap_or(0.0)
    }

    // Histogram operations

    /// Record a value in a histogram.
    pub async fn histogram_record(&self, name: &str, value: f64) {
        let mut histograms = self.histograms.write().await;
        histograms
            .entry(name.to_string())
            .or_insert_with(|| Histogram::new(default_buckets()))
            .record(value);
    }

    /// Get histogram stats.
    pub async fn histogram_stats(&self, name: &str) -> Option<HistogramStats> {
        self.histograms.read().await.get(name).map(Histogram::stats)
    }

    // Timer operations

    /// Start a timer and return the start instant.
    pub fn timer_start(&self) -> Instant {
        Instant::now()
    }

    /// Record a duration from an instant.
    pub async fn timer_record_from(&self, name: &str, start: Instant) {
        let duration = start.elapsed();
        self.timer_record(name, duration).await;
    }

    /// Record a duration.
    pub async fn timer_record(&self, name: &str, duration: Duration) {
        let mut timers = self.timers.write().await;
        timers
            .entry(name.to_string())
            .or_insert_with(Timer::new)
            .record(duration);
    }

    /// Get timer stats.
    pub async fn timer_stats(&self, name: &str) -> Option<TimerStats> {
        self.timers.read().await.get(name).map(Timer::stats)
    }

    // Export

    /// Export all metrics.
    pub async fn export(&self) -> MetricsSnapshot {
        let counters = self.counters.read().await;
        let gauges = self.gauges.read().await;
        let histograms = self.histograms.read().await;
        let timers = self.timers.read().await;
        let labels = self.labels.read().await;

        MetricsSnapshot {
            timestamp: current_timestamp(),
            uptime_secs: self.start_time.elapsed().as_secs(),
            labels: labels.clone(),
            counters: counters
                .iter()
                .map(|(k, v)| (k.clone(), v.value()))
                .collect(),
            gauges: gauges.iter().map(|(k, v)| (k.clone(), v.value())).collect(),
            histograms: histograms
                .iter()
                .map(|(k, v)| (k.clone(), v.stats()))
                .collect(),
            timers: timers.iter().map(|(k, v)| (k.clone(), v.stats())).collect(),
        }
    }

    /// Export to JSON.
    pub async fn export_json(&self) -> String {
        let snapshot = self.export().await;
        serde_json::to_string_pretty(&snapshot).unwrap_or_default()
    }

    /// Export to Prometheus format.
    pub async fn export_prometheus(&self) -> String {
        let snapshot = self.export().await;
        let mut output = String::new();

        // Counters
        for (name, value) in &snapshot.counters {
            output.push_str(&format!("# TYPE {name} counter\n"));
            output.push_str(&format!("{name} {value}\n"));
        }

        // Gauges
        for (name, value) in &snapshot.gauges {
            output.push_str(&format!("# TYPE {name} gauge\n"));
            output.push_str(&format!("{name} {value}\n"));
        }

        // Histograms
        for (name, stats) in &snapshot.histograms {
            output.push_str(&format!("# TYPE {name} histogram\n"));
            output.push_str(&format!("{}_count {}\n", name, stats.count));
            output.push_str(&format!("{}_sum {}\n", name, stats.sum));
            for (bucket, count) in &stats.buckets {
                output.push_str(&format!("{name}_bucket{{le=\"{bucket}\"}} {count}\n"));
            }
        }

        // Timers (as histograms)
        for (name, stats) in &snapshot.timers {
            output.push_str(&format!("# TYPE {name}_seconds histogram\n"));
            output.push_str(&format!("{}_seconds_count {}\n", name, stats.count));
            output.push_str(&format!(
                "{}_seconds_sum {}\n",
                name,
                stats.total_ms as f64 / 1000.0
            ));
        }

        output
    }

    /// Reset all metrics.
    pub async fn reset(&self) {
        self.counters.write().await.clear();
        self.gauges.write().await.clear();
        self.histograms.write().await.clear();
        self.timers.write().await.clear();
    }

    /// Get uptime.
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// A counter metric (monotonically increasing).
#[derive(Debug)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    /// Create a new counter.
    pub fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    /// Add to the counter.
    pub fn add(&self, n: u64) {
        self.value.fetch_add(n, Ordering::Relaxed);
    }

    /// Get current value.
    pub fn value(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Reset the counter.
    pub fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

/// A gauge metric (can go up or down).
#[derive(Debug)]
pub struct Gauge {
    value: std::sync::RwLock<f64>,
}

impl Gauge {
    /// Create a new gauge.
    pub fn new() -> Self {
        Self {
            value: std::sync::RwLock::new(0.0),
        }
    }

    /// Set the gauge value.
    pub fn set(&self, value: f64) {
        *self.value.write().unwrap() = value;
    }

    /// Add to the gauge.
    pub fn add(&self, delta: f64) {
        *self.value.write().unwrap() += delta;
    }

    /// Get current value.
    pub fn value(&self) -> f64 {
        *self.value.read().unwrap()
    }
}

impl Default for Gauge {
    fn default() -> Self {
        Self::new()
    }
}

/// A histogram metric.
#[derive(Debug)]
pub struct Histogram {
    buckets: Vec<f64>,
    counts: Vec<AtomicU64>,
    sum: std::sync::RwLock<f64>,
    count: AtomicU64,
    min: std::sync::RwLock<f64>,
    max: std::sync::RwLock<f64>,
}

impl Histogram {
    /// Create a new histogram with given bucket boundaries.
    pub fn new(buckets: Vec<f64>) -> Self {
        let counts = buckets.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            buckets,
            counts,
            sum: std::sync::RwLock::new(0.0),
            count: AtomicU64::new(0),
            min: std::sync::RwLock::new(f64::MAX),
            max: std::sync::RwLock::new(f64::MIN),
        }
    }

    /// Record a value.
    pub fn record(&self, value: f64) {
        // Update sum and count
        *self.sum.write().unwrap() += value;
        self.count.fetch_add(1, Ordering::Relaxed);

        // Update min/max
        {
            let mut min = self.min.write().unwrap();
            if value < *min {
                *min = value;
            }
        }
        {
            let mut max = self.max.write().unwrap();
            if value > *max {
                *max = value;
            }
        }

        // Update bucket counts
        for (i, bucket) in self.buckets.iter().enumerate() {
            if value <= *bucket {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Get histogram statistics.
    pub fn stats(&self) -> HistogramStats {
        let count = self.count.load(Ordering::Relaxed);
        let sum = *self.sum.read().unwrap();
        let min = *self.min.read().unwrap();
        let max = *self.max.read().unwrap();

        let buckets: Vec<_> = self
            .buckets
            .iter()
            .zip(self.counts.iter())
            .map(|(b, c)| (*b, c.load(Ordering::Relaxed)))
            .collect();

        HistogramStats {
            count,
            sum,
            min: if count > 0 { min } else { 0.0 },
            max: if count > 0 { max } else { 0.0 },
            mean: if count > 0 { sum / count as f64 } else { 0.0 },
            buckets,
        }
    }
}

/// Histogram statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramStats {
    /// Total count.
    pub count: u64,
    /// Sum of all values.
    pub sum: f64,
    /// Minimum value.
    pub min: f64,
    /// Maximum value.
    pub max: f64,
    /// Mean value.
    pub mean: f64,
    /// Bucket counts.
    pub buckets: Vec<(f64, u64)>,
}

/// A timer metric.
#[derive(Debug)]
pub struct Timer {
    count: AtomicU64,
    total_ns: AtomicU64,
    min_ns: std::sync::RwLock<u64>,
    max_ns: std::sync::RwLock<u64>,
    histogram: Histogram,
}

impl Timer {
    /// Create a new timer.
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            total_ns: AtomicU64::new(0),
            min_ns: std::sync::RwLock::new(u64::MAX),
            max_ns: std::sync::RwLock::new(0),
            histogram: Histogram::new(timing_buckets()),
        }
    }

    /// Start timing.
    pub fn start(&self) -> TimerGuard<'_> {
        TimerGuard {
            start: Instant::now(),
            timer: self,
        }
    }

    /// Record a duration.
    pub fn record(&self, duration: Duration) {
        let ns = duration.as_nanos() as u64;

        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(ns, Ordering::Relaxed);

        {
            let mut min = self.min_ns.write().unwrap();
            if ns < *min {
                *min = ns;
            }
        }
        {
            let mut max = self.max_ns.write().unwrap();
            if ns > *max {
                *max = ns;
            }
        }

        // Record in histogram (in milliseconds)
        self.histogram.record(duration.as_secs_f64() * 1000.0);
    }

    /// Get timer statistics.
    pub fn stats(&self) -> TimerStats {
        let count = self.count.load(Ordering::Relaxed);
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        let min_ns = *self.min_ns.read().unwrap();
        let max_ns = *self.max_ns.read().unwrap();

        TimerStats {
            count,
            total_ms: total_ns / 1_000_000,
            min_ms: if count > 0 { min_ns / 1_000_000 } else { 0 },
            max_ms: if count > 0 { max_ns / 1_000_000 } else { 0 },
            mean_ms: total_ns
                .checked_div(count)
                .map(|v| v / 1_000_000)
                .unwrap_or(0),
        }
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

/// Timer guard for RAII-style timing.
pub struct TimerGuard<'a> {
    start: Instant,
    timer: &'a Timer,
}

impl<'a> TimerGuard<'a> {
    /// Stop the timer and record duration.
    pub fn stop(self) -> Duration {
        let duration = self.start.elapsed();
        self.timer.record(duration);
        duration
    }
}

impl<'a> Drop for TimerGuard<'a> {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        self.timer.record(duration);
    }
}

/// Timer statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimerStats {
    /// Total count.
    pub count: u64,
    /// Total time in milliseconds.
    pub total_ms: u64,
    /// Minimum time in milliseconds.
    pub min_ms: u64,
    /// Maximum time in milliseconds.
    pub max_ms: u64,
    /// Mean time in milliseconds.
    pub mean_ms: u64,
}

/// Metrics snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Timestamp.
    pub timestamp: u64,
    /// Uptime in seconds.
    pub uptime_secs: u64,
    /// Global labels.
    pub labels: HashMap<String, String>,
    /// Counter values.
    pub counters: HashMap<String, u64>,
    /// Gauge values.
    pub gauges: HashMap<String, f64>,
    /// Histogram stats.
    pub histograms: HashMap<String, HistogramStats>,
    /// Timer stats.
    pub timers: HashMap<String, TimerStats>,
}

/// Default histogram buckets.
fn default_buckets() -> Vec<f64> {
    vec![
        0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ]
}

/// Timing histogram buckets (in milliseconds).
fn timing_buckets() -> Vec<f64> {
    vec![
        1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0,
    ]
}

/// Get current timestamp.
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pre-defined metric names.
pub mod metric_names {
    // Counters
    pub const REQUESTS_TOTAL: &str = "cortex_requests_total";
    pub const REQUESTS_FAILED: &str = "cortex_requests_failed";
    pub const TOKENS_INPUT: &str = "cortex_tokens_input_total";
    pub const TOKENS_OUTPUT: &str = "cortex_tokens_output_total";
    pub const TOOL_CALLS: &str = "cortex_tool_calls_total";
    pub const TOOL_ERRORS: &str = "cortex_tool_errors_total";
    pub const APPROVALS_GRANTED: &str = "cortex_approvals_granted";
    pub const APPROVALS_DENIED: &str = "cortex_approvals_denied";

    // Gauges
    pub const ACTIVE_SESSIONS: &str = "cortex_active_sessions";
    pub const CONTEXT_TOKENS: &str = "cortex_context_tokens";
    pub const PENDING_APPROVALS: &str = "cortex_pending_approvals";
    pub const MCP_SERVERS_ACTIVE: &str = "cortex_mcp_servers_active";

    // Histograms
    pub const RESPONSE_SIZE: &str = "cortex_response_size_bytes";
    pub const CONTEXT_SIZE: &str = "cortex_context_size_tokens";

    // Timers
    pub const REQUEST_DURATION: &str = "cortex_request_duration";
    pub const TOOL_DURATION: &str = "cortex_tool_duration";
    pub const MODEL_LATENCY: &str = "cortex_model_latency";
    pub const STREAM_FIRST_TOKEN: &str = "cortex_stream_first_token";
}

/// Agent metrics helper.
pub struct AgentMetricsHelper {
    collector: Arc<MetricsCollector>,
}

impl AgentMetricsHelper {
    /// Create a new helper.
    pub fn new(collector: Arc<MetricsCollector>) -> Self {
        Self { collector }
    }

    /// Record a request.
    pub async fn record_request(&self, success: bool, duration: Duration) {
        self.collector.increment(metric_names::REQUESTS_TOTAL).await;
        if !success {
            self.collector
                .increment(metric_names::REQUESTS_FAILED)
                .await;
        }
        self.collector
            .timer_record(metric_names::REQUEST_DURATION, duration)
            .await;
    }

    /// Record token usage.
    pub async fn record_tokens(&self, input: u64, output: u64) {
        self.collector
            .increment_by(metric_names::TOKENS_INPUT, input)
            .await;
        self.collector
            .increment_by(metric_names::TOKENS_OUTPUT, output)
            .await;
    }

    /// Record tool call.
    pub async fn record_tool_call(&self, success: bool, duration: Duration) {
        self.collector.increment(metric_names::TOOL_CALLS).await;
        if !success {
            self.collector.increment(metric_names::TOOL_ERRORS).await;
        }
        self.collector
            .timer_record(metric_names::TOOL_DURATION, duration)
            .await;
    }

    /// Record approval decision.
    pub async fn record_approval(&self, granted: bool) {
        if granted {
            self.collector
                .increment(metric_names::APPROVALS_GRANTED)
                .await;
        } else {
            self.collector
                .increment(metric_names::APPROVALS_DENIED)
                .await;
        }
    }

    /// Update active sessions.
    pub async fn set_active_sessions(&self, count: u64) {
        self.collector
            .gauge_set(metric_names::ACTIVE_SESSIONS, count as f64)
            .await;
    }

    /// Record model latency.
    pub async fn record_model_latency(&self, duration: Duration) {
        self.collector
            .timer_record(metric_names::MODEL_LATENCY, duration)
            .await;
    }

    /// Record time to first token.
    pub async fn record_first_token(&self, duration: Duration) {
        self.collector
            .timer_record(metric_names::STREAM_FIRST_TOKEN, duration)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_counter() {
        let collector = MetricsCollector::default_collector();

        collector.increment("test_counter").await;
        collector.increment("test_counter").await;
        collector.increment_by("test_counter", 5).await;

        assert_eq!(collector.counter_value("test_counter").await, 7);
    }

    #[tokio::test]
    async fn test_gauge() {
        let collector = MetricsCollector::default_collector();

        collector.gauge_set("test_gauge", 10.0).await;
        assert_eq!(collector.gauge_value("test_gauge").await, 10.0);

        collector.gauge_add("test_gauge", 5.0).await;
        assert_eq!(collector.gauge_value("test_gauge").await, 15.0);

        collector.gauge_dec("test_gauge").await;
        assert_eq!(collector.gauge_value("test_gauge").await, 14.0);
    }

    #[tokio::test]
    async fn test_histogram() {
        let collector = MetricsCollector::default_collector();

        for i in 1..=10 {
            collector.histogram_record("test_hist", i as f64).await;
        }

        let stats = collector.histogram_stats("test_hist").await.unwrap();
        assert_eq!(stats.count, 10);
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 10.0);
        assert_eq!(stats.mean, 5.5);
    }

    #[tokio::test]
    async fn test_timer() {
        let collector = MetricsCollector::default_collector();

        collector
            .timer_record("test_timer", Duration::from_millis(100))
            .await;
        collector
            .timer_record("test_timer", Duration::from_millis(200))
            .await;

        let stats = collector.timer_stats("test_timer").await.unwrap();
        assert_eq!(stats.count, 2);
        assert!(stats.total_ms >= 300);
    }

    #[tokio::test]
    async fn test_export() {
        let collector = MetricsCollector::default_collector();

        collector.increment("requests").await;
        collector.gauge_set("active", 5.0).await;

        let snapshot = collector.export().await;
        assert_eq!(snapshot.counters.get("requests"), Some(&1));
        assert_eq!(snapshot.gauges.get("active"), Some(&5.0));
    }

    #[tokio::test]
    async fn test_prometheus_export() {
        let collector = MetricsCollector::default_collector();

        collector.increment("http_requests_total").await;
        collector.gauge_set("temperature", 25.5).await;

        let prometheus = collector.export_prometheus().await;
        assert!(prometheus.contains("http_requests_total 1"));
        assert!(prometheus.contains("temperature 25.5"));
    }

    #[test]
    fn test_timer_guard() {
        let timer = Timer::new();

        {
            let _guard = timer.start();
            std::thread::sleep(Duration::from_millis(10));
        }

        let stats = timer.stats();
        assert_eq!(stats.count, 1);
        assert!(stats.total_ms >= 10);
    }
}
