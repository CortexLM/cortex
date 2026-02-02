//! Number formatting utilities.

/// Format a number with thousand separators based on locale.
pub fn format_with_separators(num: i64) -> String {
    let s = num.abs().to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);

    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(*c);
    }

    if num < 0 {
        format!("-{result}")
    } else {
        result
    }
}

/// Format bytes as human-readable string.
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Format duration as human-readable string.
pub fn format_duration_short(millis: u64) -> String {
    if millis < 1000 {
        format!("{millis}ms")
    } else if millis < 60_000 {
        format!("{:.1}s", millis as f64 / 1000.0)
    } else if millis < 3_600_000 {
        let mins = millis / 60_000;
        let secs = (millis % 60_000) / 1000;
        format!("{mins}m {secs}s")
    } else {
        let hours = millis / 3_600_000;
        let mins = (millis % 3_600_000) / 60_000;
        format!("{hours}h {mins}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_with_separators() {
        assert_eq!(format_with_separators(0), "0");
        assert_eq!(format_with_separators(123), "123");
        assert_eq!(format_with_separators(1234), "1,234");
        assert_eq!(format_with_separators(1234567), "1,234,567");
        assert_eq!(format_with_separators(-1234), "-1,234");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }

    #[test]
    fn test_format_duration_short() {
        assert_eq!(format_duration_short(500), "500ms");
        assert_eq!(format_duration_short(1500), "1.5s");
        assert_eq!(format_duration_short(90000), "1m 30s");
        assert_eq!(format_duration_short(3660000), "1h 1m");
    }
}
