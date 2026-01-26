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
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
    } else if s.ends_with('m') {
        s.trim_end_matches('m')
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else if s.ends_with('h') {
        s.trim_end_matches('h')
            .parse::<u64>()
            .ok()
            .map(|h| Duration::from_secs(h * 3600))
    } else {
        // Try parsing as milliseconds number
        s.parse::<u64>().ok().map(Duration::from_millis)
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
}
