//! Rate limits display formatting.

use crate::{RateLimitInfo, UsageStats};

/// Display configuration for rate limits.
pub struct RateLimitDisplay {
    /// Show requests info.
    pub show_requests: bool,
    /// Show tokens info.
    pub show_tokens: bool,
    /// Show usage stats.
    pub show_usage: bool,
    /// Use colors.
    pub use_colors: bool,
}

impl Default for RateLimitDisplay {
    fn default() -> Self {
        Self {
            show_requests: true,
            show_tokens: true,
            show_usage: true,
            use_colors: true,
        }
    }
}

/// Format rate limits for display.
pub fn format_rate_limits(
    info: &RateLimitInfo,
    usage: &UsageStats,
    config: &RateLimitDisplay,
) -> Vec<String> {
    let mut lines = Vec::new();

    // Requests
    if config.show_requests {
        if let (Some(remaining), Some(limit)) = (info.requests_remaining, info.requests_per_minute)
        {
            let percent = 100.0 * remaining as f32 / limit as f32;
            let bar = create_bar(percent, 20);
            lines.push(format!(
                "Requests: {} [{:>3.0}%] {}/{}",
                bar, percent, remaining, limit
            ));
        }
    }

    // Tokens per minute
    if config.show_tokens {
        if let (Some(remaining), Some(limit)) = (info.tokens_remaining, info.tokens_per_minute) {
            let percent = 100.0 * remaining as f32 / limit as f32;
            let bar = create_bar(percent, 20);
            lines.push(format!(
                "Tokens:   {} [{:>3.0}%] {}/{}",
                bar,
                percent,
                format_number(remaining as u64),
                format_number(limit as u64)
            ));
        }
    }

    // Daily usage
    if let (Some(used), Some(limit)) = (info.tokens_used_today, info.tokens_per_day) {
        let percent = 100.0 * used as f32 / limit as f32;
        let bar = create_bar(100.0 - percent, 20);
        lines.push(format!(
            "Daily:    {} [{:>3.0}%] {}/{}",
            bar,
            100.0 - percent,
            format_number(used),
            format_number(limit)
        ));
    }

    // Session usage
    if config.show_usage {
        lines.push(String::new());
        lines.push(format!(
            "Session: {} requests, {} tokens",
            usage.total_requests,
            format_number(usage.total_tokens())
        ));
        if usage.total_cached_tokens > 0 {
            lines.push(format!(
                "  Cached: {} tokens ({:.1}%)",
                format_number(usage.total_cached_tokens),
                100.0 * usage.total_cached_tokens as f32 / usage.total_input_tokens.max(1) as f32
            ));
        }
    }

    // Reset time
    if let Some(reset) = info.reset_at {
        let now = chrono::Utc::now();
        let duration = reset.signed_duration_since(now);
        if duration.num_seconds() > 0 {
            lines.push(format!("Resets in: {}s", duration.num_seconds()));
        }
    }

    lines
}

/// Create a progress bar.
fn create_bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

/// Format a number with K/M suffixes.
fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Format rate limits as a single line for status bar.
pub fn format_rate_limits_compact(info: &RateLimitInfo) -> String {
    let mut parts = Vec::new();

    if let (Some(remaining), Some(limit)) = (info.requests_remaining, info.requests_per_minute) {
        parts.push(format!("R:{}/{}", remaining, limit));
    }

    if let (Some(remaining), Some(limit)) = (info.tokens_remaining, info.tokens_per_minute) {
        parts.push(format!(
            "T:{}/{}",
            format_number(remaining as u64),
            format_number(limit as u64)
        ));
    }

    if parts.is_empty() {
        "Rate limits: unknown".to_string()
    } else {
        parts.join(" | ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(500), "500");
        assert_eq!(format_number(1500), "1.5K");
        assert_eq!(format_number(1500000), "1.5M");
    }

    #[test]
    fn test_create_bar() {
        let bar = create_bar(50.0, 10);
        assert_eq!(bar.chars().count(), 12); // 10 chars + 2 brackets
    }
}
