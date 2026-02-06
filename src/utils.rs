//! Shared utility functions used across the crate.

use std::time::Duration;

/// Parse a duration string (e.g., "30s", "500ms", "1m", "1h") into std::time::Duration.
///
/// Supported formats:
/// - `Nms` - milliseconds (e.g., "500ms")
/// - `Ns` - seconds (e.g., "30s")
/// - `Nm` - minutes (e.g., "5m")
/// - `Nh` - hours (e.g., "1h")
/// - Plain number - treated as milliseconds (e.g., "1000")
///
/// Returns `None` if the string cannot be parsed.
pub fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.ends_with("ms") {
        s.trim_end_matches("ms")
            .parse::<u64>()
            .ok()
            .map(Duration::from_millis)
    } else if s.ends_with('s') {
        s.trim_end_matches('s')
            .parse::<f64>()
            .ok()
            .map(Duration::from_secs_f64)
    } else if s.ends_with('m') {
        s.trim_end_matches('m')
            .parse::<f64>()
            .ok()
            .map(|m| Duration::from_secs_f64(m * 60.0))
    } else if s.ends_with('h') {
        s.trim_end_matches('h')
            .parse::<f64>()
            .ok()
            .map(|h| Duration::from_secs_f64(h * 3600.0))
    } else {
        // Try parsing as milliseconds number
        s.parse::<u64>().ok().map(Duration::from_millis)
    }
}

/// Parse a duration string with warning on invalid input.
///
/// If the string cannot be parsed, prints a warning and returns the default duration.
/// This is useful for CLI parsing where silent failures are undesirable.
pub fn parse_duration_str_or_warn(s: &str, default_secs: u64, context: &str) -> Duration {
    match parse_duration_str(s) {
        Some(d) => d,
        None => {
            eprintln!(
                "Warning: Invalid duration '{}' for {}, using default {}s",
                s, context, default_secs
            );
            Duration::from_secs(default_secs)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_milliseconds() {
        assert_eq!(
            parse_duration_str("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_duration_str("1000ms"),
            Some(Duration::from_millis(1000))
        );
    }

    #[test]
    fn test_parse_seconds() {
        assert_eq!(parse_duration_str("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration_str("1s"), Some(Duration::from_secs(1)));
    }

    #[test]
    fn test_parse_minutes() {
        assert_eq!(parse_duration_str("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration_str("1m"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_parse_hours() {
        assert_eq!(parse_duration_str("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration_str("2h"), Some(Duration::from_secs(7200)));
    }

    #[test]
    fn test_parse_plain_number() {
        assert_eq!(
            parse_duration_str("1000"),
            Some(Duration::from_millis(1000))
        );
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_duration_str(" 30s "), Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_parse_invalid() {
        assert_eq!(parse_duration_str("invalid"), None);
        assert_eq!(parse_duration_str("abc123"), None);
    }

    #[test]
    fn test_parse_duration_str_or_warn_valid() {
        assert_eq!(
            parse_duration_str_or_warn("30s", 60, "test"),
            Duration::from_secs(30)
        );
        assert_eq!(
            parse_duration_str_or_warn("500ms", 60, "test"),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn test_parse_fractional_seconds() {
        assert_eq!(
            parse_duration_str("1.5s"),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(parse_duration_str("0.5s"), Some(Duration::from_millis(500)));
    }

    #[test]
    fn test_parse_fractional_minutes() {
        assert_eq!(parse_duration_str("1.5m"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration_str("0.5m"), Some(Duration::from_secs(30)));
    }

    #[test]
    fn test_parse_duration_str_or_warn_invalid_uses_default() {
        assert_eq!(
            parse_duration_str_or_warn("invalid", 60, "test"),
            Duration::from_secs(60)
        );
        assert_eq!(
            parse_duration_str_or_warn("abc", 30, "duration"),
            Duration::from_secs(30)
        );
    }
}
