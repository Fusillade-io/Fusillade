use crate::engine::control::{ControlCommand, ControlState};
use crate::stats::ShardedAggregator;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) mod ui;

/// Input mode for the TUI
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    RampInput(String),
    TagInput(String),
}

/// Snapshot of stats for the TUI to render
pub struct TuiSnapshot {
    pub total_requests: usize,
    pub rps: f64,
    pub error_count: usize,
    pub error_pct: f64,
    pub avg_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub data_sent_bytes: u64,
    pub data_recv_bytes: u64,
    pub status_codes: Vec<(u16, usize)>,
    pub endpoints: Vec<EndpointRow>,
}

pub struct EndpointRow {
    pub name: String,
    pub reqs: usize,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub errors: usize,
}

/// Application state for the TUI
pub struct App {
    pub aggregator: Arc<ShardedAggregator>,
    pub control_tx: Sender<ControlCommand>,
    pub control_state: Arc<ControlState>,
    pub start_time: Instant,
    pub total_duration: Option<Duration>,
    pub input_mode: InputMode,
    pub endpoint_scroll: usize,
    pub snapshot: Option<TuiSnapshot>,
}

impl App {
    pub fn new(
        aggregator: Arc<ShardedAggregator>,
        control_tx: Sender<ControlCommand>,
        control_state: Arc<ControlState>,
        total_duration: Option<Duration>,
    ) -> Self {
        Self {
            aggregator,
            control_tx,
            control_state,
            start_time: Instant::now(),
            total_duration,
            input_mode: InputMode::Normal,
            endpoint_scroll: 0,
            snapshot: None,
        }
    }

    pub fn refresh_snapshot(&mut self) {
        let merged = self.aggregator.merge();
        let report = merged.to_report();
        let elapsed_secs = self.start_time.elapsed().as_secs_f64().max(0.001);
        let rps = report.total_requests as f64 / elapsed_secs;

        let error_count: usize = report.errors.values().sum();
        let error_pct = if report.total_requests > 0 {
            error_count as f64 / report.total_requests as f64 * 100.0
        } else {
            0.0
        };

        let mut status_codes: Vec<(u16, usize)> = report.status_codes.into_iter().collect();
        status_codes.sort_by_key(|(code, _)| *code);

        let mut endpoints: Vec<EndpointRow> = report
            .grouped_requests
            .iter()
            .filter(|(name, _)| name.as_str() != "iteration" && name.as_str() != "iteration_total")
            .map(|(name, r)| EndpointRow {
                name: name.clone(),
                reqs: r.total_requests,
                avg_ms: r.avg_latency_ms,
                p95_ms: r.p95_latency_ms,
                errors: r.error_count,
            })
            .collect();
        endpoints.sort_by(|a, b| b.reqs.cmp(&a.reqs));

        self.snapshot = Some(TuiSnapshot {
            total_requests: report.total_requests,
            rps,
            error_count,
            error_pct,
            avg_ms: report.avg_latency_ms,
            min_ms: report.min_latency_ms as f64,
            max_ms: report.max_latency_ms as f64,
            p50_ms: report.p50_latency_ms,
            p90_ms: report.p90_latency_ms,
            p95_ms: report.p95_latency_ms,
            p99_ms: report.p99_latency_ms,
            data_sent_bytes: report.total_data_sent,
            data_recv_bytes: report.total_data_received,
            status_codes,
            endpoints,
        });
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> bool {
        match &self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::RampInput(_) => self.handle_ramp_key(key),
            InputMode::TagInput(_) => self.handle_tag_key(key),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                let _ = self.control_tx.send(ControlCommand::Stop);
                return true;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.control_tx.send(ControlCommand::Stop);
                return true;
            }
            KeyCode::Char('p') => {
                if self.control_state.is_paused() {
                    let _ = self.control_tx.send(ControlCommand::Resume);
                } else {
                    let _ = self.control_tx.send(ControlCommand::Pause);
                }
            }
            KeyCode::Char('s') => {
                let _ = self.control_tx.send(ControlCommand::Stop);
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let current = self.control_state.get_target_workers();
                let _ = self.control_tx.send(ControlCommand::Ramp(current + 10));
            }
            KeyCode::Char('-') => {
                let current = self.control_state.get_target_workers();
                let new = current.saturating_sub(10).max(1);
                let _ = self.control_tx.send(ControlCommand::Ramp(new));
            }
            KeyCode::Char('r') => {
                self.input_mode = InputMode::RampInput(String::new());
            }
            KeyCode::Char('a') => {
                // Return to automatic schedule
                let _ = self.control_tx.send(ControlCommand::Ramp(0));
            }
            KeyCode::Char('t') => {
                self.input_mode = InputMode::TagInput(String::new());
            }
            KeyCode::Up => {
                self.endpoint_scroll = self.endpoint_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.endpoint_scroll += 1;
            }
            _ => {}
        }
        false
    }

    fn handle_ramp_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if let InputMode::RampInput(ref s) = self.input_mode {
                    if let Ok(n) = s.parse::<usize>() {
                        let _ = self.control_tx.send(ControlCommand::Ramp(n));
                    }
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let InputMode::RampInput(ref mut s) = self.input_mode {
                    s.push(c);
                }
            }
            KeyCode::Backspace => {
                if let InputMode::RampInput(ref mut s) = self.input_mode {
                    s.pop();
                }
            }
            _ => {}
        }
        false
    }

    fn handle_tag_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if let InputMode::TagInput(ref s) = self.input_mode {
                    if let Some((k, v)) = s.split_once('=') {
                        let _ = self
                            .control_tx
                            .send(ControlCommand::Tag(k.to_string(), v.to_string()));
                    }
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char(c) => {
                if let InputMode::TagInput(ref mut s) = self.input_mode {
                    s.push(c);
                }
            }
            KeyCode::Backspace => {
                if let InputMode::TagInput(ref mut s) = self.input_mode {
                    s.pop();
                }
            }
            _ => {}
        }
        false
    }
}

/// Helper to create a KeyEvent for testing
#[cfg(test)]
fn key_event(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[cfg(test)]
fn key_event_ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Guard that restores terminal state on drop (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        crate::bridge::TUI_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Run the TUI on the current thread. Blocks until the test completes or user quits.
pub fn run_tui(
    aggregator: Arc<ShardedAggregator>,
    control_tx: Sender<ControlCommand>,
    control_state: Arc<ControlState>,
    total_duration: Option<Duration>,
) -> io::Result<()> {
    crate::bridge::TUI_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Guard ensures terminal is restored even on panic
    let _guard = TerminalGuard;

    let mut app = App::new(
        aggregator,
        control_tx,
        control_state.clone(),
        total_duration,
    );
    let tick_rate = Duration::from_secs(1);
    let mut last_tick = Instant::now();

    // Initial snapshot
    app.refresh_snapshot();

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Only handle key press events, ignore release/repeat
                if key.kind == KeyEventKind::Press && app.handle_key(key) {
                    break;
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.refresh_snapshot();
            last_tick = Instant::now();
        }

        // Check if test has stopped
        if control_state.is_stopped() {
            // One final refresh
            app.refresh_snapshot();
            terminal.draw(|f| ui::draw(f, &app))?;
            // Brief pause so user can see final state
            std::thread::sleep(Duration::from_millis(500));
            break;
        }
    }

    terminal.show_cursor()?;
    // _guard drop handles: TUI_ACTIVE = false, disable_raw_mode, LeaveAlternateScreen
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::control::{ControlCommand, ControlState};
    use crate::stats::{Metric, RequestTimings, ShardedAggregator};
    use crossterm::event::KeyCode;
    use std::collections::HashMap;
    use std::sync::mpsc;

    fn make_app() -> (App, mpsc::Receiver<ControlCommand>) {
        let agg = Arc::new(ShardedAggregator::new(4));
        let (tx, rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(100));
        let app = App::new(agg, tx, cs, Some(Duration::from_secs(120)));
        (app, rx)
    }

    // ── App creation ─────────────────────────────────────────────

    #[test]
    fn test_app_initial_state() {
        let (app, _rx) = make_app();
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.endpoint_scroll, 0);
        assert!(app.snapshot.is_none());
        assert_eq!(app.total_duration, Some(Duration::from_secs(120)));
    }

    #[test]
    fn test_app_no_duration() {
        let agg = Arc::new(ShardedAggregator::new(4));
        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(1));
        let app = App::new(agg, tx, cs, None);
        assert!(app.total_duration.is_none());
    }

    // ── Snapshot refresh ─────────────────────────────────────────

    #[test]
    fn test_refresh_snapshot_empty() {
        let (mut app, _rx) = make_app();
        app.refresh_snapshot();
        let snap = app.snapshot.as_ref().unwrap();
        assert_eq!(snap.total_requests, 0);
        assert_eq!(snap.error_count, 0);
        assert_eq!(snap.error_pct, 0.0);
        assert!(snap.status_codes.is_empty());
        assert!(snap.endpoints.is_empty());
    }

    #[test]
    fn test_refresh_snapshot_with_data() {
        let agg = Arc::new(ShardedAggregator::new(4));
        // Add some metrics
        agg.add(
            0,
            Metric::Request {
                name: "GET /api/users".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(50),
                    response_size: 1024,
                    request_size: 128,
                    ..Default::default()
                },
                status: 200,
                error: None,
                tags: HashMap::new(),
            },
        );
        agg.add(
            1,
            Metric::Request {
                name: "POST /api/login".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(100),
                    response_size: 512,
                    request_size: 256,
                    ..Default::default()
                },
                status: 500,
                error: Some("Internal Server Error".to_string()),
                tags: HashMap::new(),
            },
        );

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(10));
        let mut app = App::new(agg, tx, cs, None);
        app.refresh_snapshot();

        let snap = app.snapshot.as_ref().unwrap();
        assert_eq!(snap.total_requests, 2);
        assert_eq!(snap.error_count, 1);
        assert!(snap.error_pct > 0.0);
        assert_eq!(snap.data_sent_bytes, 128 + 256);
        assert_eq!(snap.data_recv_bytes, 1024 + 512);
        assert_eq!(snap.status_codes.len(), 2);
        assert_eq!(snap.endpoints.len(), 2);
        // Sorted by request count descending (both have 1, so order is stable by name)
        assert!(snap.endpoints.iter().any(|e| e.name == "GET /api/users"));
        assert!(snap.endpoints.iter().any(|e| e.name == "POST /api/login"));
    }

    #[test]
    fn test_snapshot_filters_iteration_metrics() {
        let agg = Arc::new(ShardedAggregator::new(4));
        // Add real endpoint
        agg.add(
            0,
            Metric::Request {
                name: "GET /api/data".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(10),
                    ..Default::default()
                },
                status: 200,
                error: None,
                tags: HashMap::new(),
            },
        );
        // Add iteration pseudo-metrics
        agg.add(
            0,
            Metric::Request {
                name: "iteration".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(10),
                    ..Default::default()
                },
                status: 0,
                error: None,
                tags: HashMap::new(),
            },
        );
        agg.add(
            0,
            Metric::Request {
                name: "iteration_total".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(15),
                    ..Default::default()
                },
                status: 0,
                error: None,
                tags: HashMap::new(),
            },
        );

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(1));
        let mut app = App::new(agg, tx, cs, None);
        app.refresh_snapshot();

        let snap = app.snapshot.as_ref().unwrap();
        // Only the real endpoint should appear
        assert_eq!(snap.endpoints.len(), 1);
        assert_eq!(snap.endpoints[0].name, "GET /api/data");
    }

    #[test]
    fn test_snapshot_status_codes_sorted() {
        let agg = Arc::new(ShardedAggregator::new(4));
        for (status, name) in [(500, "a"), (200, "b"), (404, "c")] {
            agg.add(
                0,
                Metric::Request {
                    name: name.to_string(),
                    timings: RequestTimings {
                        duration: tokio::time::Duration::from_millis(10),
                        ..Default::default()
                    },
                    status,
                    error: if status >= 400 {
                        Some("err".to_string())
                    } else {
                        None
                    },
                    tags: HashMap::new(),
                },
            );
        }

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(1));
        let mut app = App::new(agg, tx, cs, None);
        app.refresh_snapshot();

        let codes: Vec<u16> = app
            .snapshot
            .as_ref()
            .unwrap()
            .status_codes
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(codes, vec![200, 404, 500]);
    }

    #[test]
    fn test_snapshot_endpoints_sorted_by_reqs_desc() {
        let agg = Arc::new(ShardedAggregator::new(4));
        // Add 3 reqs to endpoint A, 1 to B
        for _ in 0..3 {
            agg.add(
                0,
                Metric::Request {
                    name: "A".to_string(),
                    timings: RequestTimings {
                        duration: tokio::time::Duration::from_millis(10),
                        ..Default::default()
                    },
                    status: 200,
                    error: None,
                    tags: HashMap::new(),
                },
            );
        }
        agg.add(
            0,
            Metric::Request {
                name: "B".to_string(),
                timings: RequestTimings {
                    duration: tokio::time::Duration::from_millis(10),
                    ..Default::default()
                },
                status: 200,
                error: None,
                tags: HashMap::new(),
            },
        );

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(1));
        let mut app = App::new(agg, tx, cs, None);
        app.refresh_snapshot();

        let names: Vec<&str> = app
            .snapshot
            .as_ref()
            .unwrap()
            .endpoints
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["A", "B"]);
    }

    // ── Normal mode key handling ─────────────────────────────────

    #[test]
    fn test_key_q_quits() {
        let (mut app, rx) = make_app();
        let quit = app.handle_key(key_event(KeyCode::Char('q')));
        assert!(quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Stop));
    }

    #[test]
    fn test_key_esc_quits() {
        let (mut app, rx) = make_app();
        let quit = app.handle_key(key_event(KeyCode::Esc));
        assert!(quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Stop));
    }

    #[test]
    fn test_key_ctrl_c_quits() {
        let (mut app, rx) = make_app();
        let quit = app.handle_key(key_event_ctrl(KeyCode::Char('c')));
        assert!(quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Stop));
    }

    #[test]
    fn test_key_s_stops() {
        let (mut app, rx) = make_app();
        let quit = app.handle_key(key_event(KeyCode::Char('s')));
        assert!(!quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Stop));
    }

    #[test]
    fn test_key_p_toggles_pause() {
        let (mut app, rx) = make_app();

        // Not paused -> sends Pause
        let quit = app.handle_key(key_event(KeyCode::Char('p')));
        assert!(!quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Pause));

        // Simulate paused state
        app.control_state.pause();
        let quit = app.handle_key(key_event(KeyCode::Char('p')));
        assert!(!quit);
        assert!(matches!(rx.recv().unwrap(), ControlCommand::Resume));
    }

    #[test]
    fn test_key_plus_increases_workers() {
        let (mut app, rx) = make_app();
        // Initial workers: 100
        app.handle_key(key_event(KeyCode::Char('+')));
        match rx.recv().unwrap() {
            ControlCommand::Ramp(n) => assert_eq!(n, 110),
            other => panic!("Expected Ramp, got {:?}", other),
        }
    }

    #[test]
    fn test_key_equals_increases_workers() {
        let (mut app, rx) = make_app();
        app.handle_key(key_event(KeyCode::Char('=')));
        match rx.recv().unwrap() {
            ControlCommand::Ramp(n) => assert_eq!(n, 110),
            other => panic!("Expected Ramp, got {:?}", other),
        }
    }

    #[test]
    fn test_key_minus_decreases_workers() {
        let (mut app, rx) = make_app();
        app.handle_key(key_event(KeyCode::Char('-')));
        match rx.recv().unwrap() {
            ControlCommand::Ramp(n) => assert_eq!(n, 90),
            other => panic!("Expected Ramp, got {:?}", other),
        }
    }

    #[test]
    fn test_key_minus_clamps_to_one() {
        let agg = Arc::new(ShardedAggregator::new(4));
        let (tx, rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(5)); // Only 5 workers
        let mut app = App::new(agg, tx, cs, None);

        app.handle_key(key_event(KeyCode::Char('-')));
        match rx.recv().unwrap() {
            ControlCommand::Ramp(n) => assert_eq!(n, 1), // 5 - 10 = 0, clamped to 1
            other => panic!("Expected Ramp, got {:?}", other),
        }
    }

    #[test]
    fn test_key_r_enters_ramp_mode() {
        let (mut app, _rx) = make_app();
        app.handle_key(key_event(KeyCode::Char('r')));
        assert_eq!(app.input_mode, InputMode::RampInput(String::new()));
    }

    #[test]
    fn test_key_t_enters_tag_mode() {
        let (mut app, _rx) = make_app();
        app.handle_key(key_event(KeyCode::Char('t')));
        assert_eq!(app.input_mode, InputMode::TagInput(String::new()));
    }

    #[test]
    fn test_arrow_keys_scroll() {
        let (mut app, _rx) = make_app();
        assert_eq!(app.endpoint_scroll, 0);

        app.handle_key(key_event(KeyCode::Down));
        assert_eq!(app.endpoint_scroll, 1);

        app.handle_key(key_event(KeyCode::Down));
        assert_eq!(app.endpoint_scroll, 2);

        app.handle_key(key_event(KeyCode::Up));
        assert_eq!(app.endpoint_scroll, 1);

        // Can't scroll past 0
        app.handle_key(key_event(KeyCode::Up));
        app.handle_key(key_event(KeyCode::Up));
        assert_eq!(app.endpoint_scroll, 0);
    }

    #[test]
    fn test_unknown_key_does_nothing() {
        let (mut app, _rx) = make_app();
        let quit = app.handle_key(key_event(KeyCode::Char('z')));
        assert!(!quit);
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    // ── Ramp input mode ──────────────────────────────────────────

    #[test]
    fn test_ramp_input_typing() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::RampInput(String::new());

        app.handle_key(key_event(KeyCode::Char('5')));
        assert_eq!(app.input_mode, InputMode::RampInput("5".to_string()));

        app.handle_key(key_event(KeyCode::Char('0')));
        assert_eq!(app.input_mode, InputMode::RampInput("50".to_string()));

        app.handle_key(key_event(KeyCode::Char('0')));
        assert_eq!(app.input_mode, InputMode::RampInput("500".to_string()));
    }

    #[test]
    fn test_ramp_input_backspace() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::RampInput("123".to_string());

        app.handle_key(key_event(KeyCode::Backspace));
        assert_eq!(app.input_mode, InputMode::RampInput("12".to_string()));

        app.handle_key(key_event(KeyCode::Backspace));
        assert_eq!(app.input_mode, InputMode::RampInput("1".to_string()));

        app.handle_key(key_event(KeyCode::Backspace));
        assert_eq!(app.input_mode, InputMode::RampInput(String::new()));

        // Backspace on empty string is fine
        app.handle_key(key_event(KeyCode::Backspace));
        assert_eq!(app.input_mode, InputMode::RampInput(String::new()));
    }

    #[test]
    fn test_ramp_input_submit() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::RampInput("250".to_string());

        let quit = app.handle_key(key_event(KeyCode::Enter));
        assert!(!quit);
        assert_eq!(app.input_mode, InputMode::Normal);
        match rx.recv().unwrap() {
            ControlCommand::Ramp(n) => assert_eq!(n, 250),
            other => panic!("Expected Ramp(250), got {:?}", other),
        }
    }

    #[test]
    fn test_ramp_input_submit_empty() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::RampInput(String::new());

        app.handle_key(key_event(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
        // No command sent for empty/invalid input
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_ramp_input_escape() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::RampInput("123".to_string());

        app.handle_key(key_event(KeyCode::Esc));
        assert_eq!(app.input_mode, InputMode::Normal);
        // No command sent
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_ramp_ignores_non_digit() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::RampInput("12".to_string());

        app.handle_key(key_event(KeyCode::Char('a')));
        assert_eq!(app.input_mode, InputMode::RampInput("12".to_string()));
    }

    // ── Tag input mode ───────────────────────────────────────────

    #[test]
    fn test_tag_input_typing() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::TagInput(String::new());

        for c in "env=prod".chars() {
            app.handle_key(key_event(KeyCode::Char(c)));
        }
        assert_eq!(app.input_mode, InputMode::TagInput("env=prod".to_string()));
    }

    #[test]
    fn test_tag_input_backspace() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::TagInput("env=".to_string());

        app.handle_key(key_event(KeyCode::Backspace));
        assert_eq!(app.input_mode, InputMode::TagInput("env".to_string()));
    }

    #[test]
    fn test_tag_input_submit() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::TagInput("region=us-east".to_string());

        let quit = app.handle_key(key_event(KeyCode::Enter));
        assert!(!quit);
        assert_eq!(app.input_mode, InputMode::Normal);
        match rx.recv().unwrap() {
            ControlCommand::Tag(k, v) => {
                assert_eq!(k, "region");
                assert_eq!(v, "us-east");
            }
            other => panic!("Expected Tag, got {:?}", other),
        }
    }

    #[test]
    fn test_tag_input_submit_no_equals() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::TagInput("invalid".to_string());

        app.handle_key(key_event(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
        // No command sent for invalid format
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_tag_input_escape() {
        let (mut app, rx) = make_app();
        app.input_mode = InputMode::TagInput("env=prod".to_string());

        app.handle_key(key_event(KeyCode::Esc));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert!(rx.try_recv().is_err());
    }

    // ── UI format helpers ────────────────────────────────────────

    #[test]
    fn test_format_duration() {
        assert_eq!(ui::format_duration(Duration::from_secs(0)), "00:00");
        assert_eq!(ui::format_duration(Duration::from_secs(5)), "00:05");
        assert_eq!(ui::format_duration(Duration::from_secs(60)), "01:00");
        assert_eq!(ui::format_duration(Duration::from_secs(61)), "01:01");
        assert_eq!(ui::format_duration(Duration::from_secs(3599)), "59:59");
        assert_eq!(ui::format_duration(Duration::from_secs(3600)), "60:00");
    }

    #[test]
    fn test_format_ms() {
        assert_eq!(ui::format_ms(0.5), "0.50ms");
        assert_eq!(ui::format_ms(0.01), "0.01ms");
        assert_eq!(ui::format_ms(1.0), "1.0ms");
        assert_eq!(ui::format_ms(23.4), "23.4ms");
        assert_eq!(ui::format_ms(999.9), "999.9ms");
        assert_eq!(ui::format_ms(1000.0), "1.0s");
        assert_eq!(ui::format_ms(1500.0), "1.5s");
        assert_eq!(ui::format_ms(60000.0), "60.0s");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(ui::format_number(0), "0");
        assert_eq!(ui::format_number(1), "1");
        assert_eq!(ui::format_number(999), "999");
        assert_eq!(ui::format_number(1000), "1.0K");
        assert_eq!(ui::format_number(1500), "1.5K");
        assert_eq!(ui::format_number(12345), "12.3K");
        assert_eq!(ui::format_number(999999), "1000.0K");
        assert_eq!(ui::format_number(1_000_000), "1.0M");
        assert_eq!(ui::format_number(2_500_000), "2.5M");
    }

    #[test]
    fn test_truncate_name() {
        assert_eq!(ui::truncate_name("short", 30), "short");
        assert_eq!(ui::truncate_name("exactly_ten", 11), "exactly_ten");
        assert_eq!(
            ui::truncate_name("this is a very long endpoint name", 20),
            "this is a very lo..."
        );
        // Exact boundary
        let s = "a".repeat(30);
        assert_eq!(ui::truncate_name(&s, 30), s);
        let s31 = "a".repeat(31);
        assert_eq!(ui::truncate_name(&s31, 30).len(), 30);
        assert!(ui::truncate_name(&s31, 30).ends_with("..."));
    }

    // ── Rendering smoke tests (ensure no panics) ─────────────────

    #[test]
    fn test_draw_no_snapshot() {
        let (app, _rx) = make_app();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_with_snapshot() {
        let agg = Arc::new(ShardedAggregator::new(4));
        for i in 0..10 {
            agg.add(
                i % 4,
                Metric::Request {
                    name: format!("GET /endpoint{}", i % 3),
                    timings: RequestTimings {
                        duration: tokio::time::Duration::from_millis(10 + i as u64 * 5),
                        response_size: 512,
                        request_size: 64,
                        ..Default::default()
                    },
                    status: if i < 8 { 200 } else { 500 },
                    error: if i >= 8 {
                        Some("err".to_string())
                    } else {
                        None
                    },
                    tags: HashMap::new(),
                },
            );
        }

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(50));
        let mut app = App::new(agg, tx, cs, Some(Duration::from_secs(300)));
        app.refresh_snapshot();

        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_paused_state() {
        let (mut app, _rx) = make_app();
        app.control_state.pause();
        app.refresh_snapshot();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_stopped_state() {
        let (mut app, _rx) = make_app();
        app.control_state.stop();
        app.refresh_snapshot();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_ramp_input_mode() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::RampInput("42".to_string());
        app.refresh_snapshot();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_tag_input_mode() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::TagInput("env=prod".to_string());
        app.refresh_snapshot();

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_small_terminal() {
        let (mut app, _rx) = make_app();
        app.refresh_snapshot();

        // Very small terminal shouldn't panic
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    #[test]
    fn test_draw_with_scrolled_endpoints() {
        let agg = Arc::new(ShardedAggregator::new(4));
        for i in 0..20 {
            agg.add(
                0,
                Metric::Request {
                    name: format!("GET /endpoint/{}", i),
                    timings: RequestTimings {
                        duration: tokio::time::Duration::from_millis(10),
                        ..Default::default()
                    },
                    status: 200,
                    error: None,
                    tags: HashMap::new(),
                },
            );
        }

        let (tx, _rx) = mpsc::channel();
        let cs = Arc::new(ControlState::new(1));
        let mut app = App::new(agg, tx, cs, None);
        app.refresh_snapshot();
        app.endpoint_scroll = 10;

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
    }

    // ── Mode transition sequences ────────────────────────────────

    #[test]
    fn test_ramp_then_tag_then_normal() {
        let (mut app, _rx) = make_app();

        // Enter ramp
        app.handle_key(key_event(KeyCode::Char('r')));
        assert!(matches!(app.input_mode, InputMode::RampInput(_)));

        // Cancel back to normal
        app.handle_key(key_event(KeyCode::Esc));
        assert_eq!(app.input_mode, InputMode::Normal);

        // Enter tag
        app.handle_key(key_event(KeyCode::Char('t')));
        assert!(matches!(app.input_mode, InputMode::TagInput(_)));

        // Submit empty tag (invalid, no command sent)
        app.handle_key(key_event(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn test_ramp_input_does_not_quit_on_q() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::RampInput(String::new());

        // 'q' should not quit in ramp mode (it's not a digit, ignored)
        let quit = app.handle_key(key_event(KeyCode::Char('q')));
        assert!(!quit);
        assert!(matches!(app.input_mode, InputMode::RampInput(_)));
    }

    #[test]
    fn test_tag_input_accepts_any_char() {
        let (mut app, _rx) = make_app();
        app.input_mode = InputMode::TagInput(String::new());

        // Tag mode accepts any character
        for c in "my-key=some_value!@#".chars() {
            app.handle_key(key_event(KeyCode::Char(c)));
        }
        assert_eq!(
            app.input_mode,
            InputMode::TagInput("my-key=some_value!@#".to_string())
        );
    }
}
