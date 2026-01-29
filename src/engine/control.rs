use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Commands that can be sent to control a running load test
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Adjust worker count to target
    Ramp(usize),
    /// Pause all workers (idle loop)
    Pause,
    /// Resume execution
    Resume,
    /// Add a tag to all subsequent metrics
    Tag(String, String),
    /// Request current status
    Status,
    /// Graceful stop
    Stop,
}

/// Shared state between controller and workers
pub struct ControlState {
    /// When true, workers should idle instead of executing
    pub paused: AtomicBool,
    /// Target worker count (for dynamic scaling)
    pub target_workers: AtomicUsize,
    /// Tags to add to all metrics - uses Mutex for thread-safe updates
    tags: Mutex<HashMap<String, String>>,
    /// Stop flag
    pub stopped: AtomicBool,
    /// Accumulated paused duration in milliseconds
    total_paused_ms: AtomicU64,
    /// Timestamp (ms since epoch-ish) when pause started, 0 if not paused
    pause_started_ms: AtomicU64,
    /// Reference instant for converting between Instant and atomic timestamps
    reference_instant: Instant,
}

impl ControlState {
    pub fn new(initial_workers: usize) -> Self {
        Self {
            paused: AtomicBool::new(false),
            target_workers: AtomicUsize::new(initial_workers),
            tags: Mutex::new(HashMap::new()),
            stopped: AtomicBool::new(false),
            total_paused_ms: AtomicU64::new(0),
            pause_started_ms: AtomicU64::new(0),
            reference_instant: Instant::now(),
        }
    }

    pub fn pause(&self) {
        let now_ms = self.reference_instant.elapsed().as_millis() as u64;
        self.pause_started_ms.store(now_ms, Ordering::SeqCst);
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        let started = self.pause_started_ms.swap(0, Ordering::SeqCst);
        if started > 0 {
            let now_ms = self.reference_instant.elapsed().as_millis() as u64;
            let paused_dur = now_ms.saturating_sub(started);
            self.total_paused_ms.fetch_add(paused_dur, Ordering::SeqCst);
        }
    }

    /// Returns total time spent paused (including current pause if active)
    pub fn total_paused(&self) -> Duration {
        let mut total = self.total_paused_ms.load(Ordering::SeqCst);
        // If currently paused, add the ongoing pause duration
        if self.is_paused() {
            let started = self.pause_started_ms.load(Ordering::SeqCst);
            if started > 0 {
                let now_ms = self.reference_instant.elapsed().as_millis() as u64;
                total += now_ms.saturating_sub(started);
            }
        }
        Duration::from_millis(total)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn set_target_workers(&self, count: usize) {
        self.target_workers.store(count, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn get_target_workers(&self) -> usize {
        self.target_workers.load(Ordering::SeqCst)
    }

    pub fn add_tag(&self, key: String, value: String) {
        // Thread-safe update using Mutex
        let mut tags = self.tags.lock();
        tags.insert(key, value);
    }

    #[allow(dead_code)]
    pub fn get_tags(&self) -> HashMap<String, String> {
        // Clone the tags to return owned value
        self.tags.lock().clone()
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

impl Default for ControlState {
    fn default() -> Self {
        Self::new(1)
    }
}

/// Parse a control command from user input
pub fn parse_control_command(input: &str) -> Option<ControlCommand> {
    let input = input.trim();
    let parts: Vec<&str> = input.split_whitespace().collect();

    if parts.is_empty() {
        return None;
    }

    match parts[0].to_lowercase().as_str() {
        "ramp" | "scale" => parts
            .get(1)?
            .parse::<usize>()
            .ok()
            .map(ControlCommand::Ramp),
        "pause" => Some(ControlCommand::Pause),
        "resume" | "unpause" => Some(ControlCommand::Resume),
        "tag" => {
            let kv = parts.get(1)?;
            let (key, value) = kv.split_once('=')?;
            Some(ControlCommand::Tag(key.to_string(), value.to_string()))
        }
        "status" | "stats" => Some(ControlCommand::Status),
        "stop" | "quit" | "exit" => Some(ControlCommand::Stop),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ramp() {
        assert!(matches!(
            parse_control_command("ramp 50"),
            Some(ControlCommand::Ramp(50))
        ));
        assert!(matches!(
            parse_control_command("scale 100"),
            Some(ControlCommand::Ramp(100))
        ));
    }

    #[test]
    fn test_parse_pause_resume() {
        assert!(matches!(
            parse_control_command("pause"),
            Some(ControlCommand::Pause)
        ));
        assert!(matches!(
            parse_control_command("resume"),
            Some(ControlCommand::Resume)
        ));
        assert!(matches!(
            parse_control_command("unpause"),
            Some(ControlCommand::Resume)
        ));
    }

    #[test]
    fn test_parse_tag() {
        match parse_control_command("tag region=us-east") {
            Some(ControlCommand::Tag(k, v)) => {
                assert_eq!(k, "region");
                assert_eq!(v, "us-east");
            }
            _ => panic!("Expected Tag command"),
        }
    }

    #[test]
    fn test_parse_stop() {
        assert!(matches!(
            parse_control_command("stop"),
            Some(ControlCommand::Stop)
        ));
        assert!(matches!(
            parse_control_command("quit"),
            Some(ControlCommand::Stop)
        ));
        assert!(matches!(
            parse_control_command("exit"),
            Some(ControlCommand::Stop)
        ));
    }

    #[test]
    fn test_control_state() {
        let state = ControlState::new(10);

        assert!(!state.is_paused());
        state.pause();
        assert!(state.is_paused());
        state.resume();
        assert!(!state.is_paused());

        assert_eq!(state.get_target_workers(), 10);
        state.set_target_workers(50);
        assert_eq!(state.get_target_workers(), 50);

        state.add_tag("env".to_string(), "prod".to_string());
        let tags = state.get_tags();
        assert_eq!(tags.get("env"), Some(&"prod".to_string()));
    }

    #[test]
    fn test_control_state_stop() {
        let state = ControlState::new(1);
        assert!(!state.is_stopped());
        state.stop();
        assert!(state.is_stopped());
    }
}
