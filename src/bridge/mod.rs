mod amqp;
mod browser;
mod check;
mod crypto;
pub mod data; // Make public so Engine can see SharedData type
mod encoding;
mod file;
mod group;
mod grpc;
mod http;
mod http_sync;
pub mod metrics;
mod mqtt;
pub mod replay;
mod sse;
mod stats;
mod test;
mod utils;
mod ws;

// --- Common Error Helpers ---
// These helpers create consistent error types for JavaScript

/// Create a network-related error
#[allow(dead_code)]
pub fn js_network_error(message: &'static str) -> rquickjs::Error {
    rquickjs::Error::new_from_js(message, "NetworkError")
}

/// Create a state-related error (e.g., "not connected")
#[allow(dead_code)]
pub fn js_state_error(message: &'static str) -> rquickjs::Error {
    rquickjs::Error::new_from_js(message, "StateError")
}

/// Create a timeout error
#[allow(dead_code)]
pub fn js_timeout_error(message: &'static str) -> rquickjs::Error {
    rquickjs::Error::new_from_js(message, "TimeoutError")
}

/// Create a type error
#[allow(dead_code)]
pub fn js_type_error(message: &'static str) -> rquickjs::Error {
    rquickjs::Error::new_from_js(message, "TypeError")
}

use crate::engine::http_client::HttpClient;
use crate::engine::io_bridge::IoBridge;
use crate::stats::{Metric, SharedAggregator};
use crossbeam_channel::Sender;
use rand::Rng;
use rquickjs::{Ctx, Function, Object, Result, Value};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Log level for console.* API
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

thread_local! {
    static LOG_LEVEL: RefCell<LogLevel> = const { RefCell::new(LogLevel::Info) };
    static LOG_FILTER: RefCell<Option<String>> = const { RefCell::new(None) };
    static CURRENT_SCENARIO: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Set the global log level for console.* API
pub fn set_log_level(level: LogLevel) {
    LOG_LEVEL.with(|l| *l.borrow_mut() = level);
}

/// Get the current log level
pub fn get_log_level() -> LogLevel {
    LOG_LEVEL.with(|l| *l.borrow())
}

/// Set the log filter (e.g., "scenario=login")
pub fn set_log_filter(filter: String) {
    LOG_FILTER.with(|f| *f.borrow_mut() = Some(filter));
}

/// Set the current scenario name for the worker thread
pub fn set_current_scenario(name: String) {
    CURRENT_SCENARIO.with(|s| *s.borrow_mut() = Some(name));
}

/// When true, suppress stdout output (TUI is active on the alternate screen)
pub static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

// Global capture errors path - set once at startup, read by HTTP module
static CAPTURE_ERRORS_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Set the capture errors file path (called once at startup)
pub fn set_capture_errors_path(path: String) {
    let _ = CAPTURE_ERRORS_PATH.set(path);
}

/// Get the capture errors file path if set
pub fn get_capture_errors_path() -> Option<&'static str> {
    CAPTURE_ERRORS_PATH.get().map(|s| s.as_str())
}

// Thread-local sleep tracker for accurate iteration timing
// This tracks cumulative sleep time during an iteration so it can be excluded from response time metrics
thread_local! {
    static SLEEP_ACCUMULATED: RefCell<Duration> = const { RefCell::new(Duration::ZERO) };
}

/// Reset the sleep tracker at the start of each iteration
pub fn reset_sleep_tracker() {
    SLEEP_ACCUMULATED.with(|s| *s.borrow_mut() = Duration::ZERO);
}

/// Get the accumulated sleep time for the current iteration
pub fn get_sleep_accumulated() -> Duration {
    SLEEP_ACCUMULATED.with(|s| *s.borrow())
}

/// Track sleep duration (called internally by sleep functions)
fn track_sleep(duration: Duration) {
    SLEEP_ACCUMULATED.with(|s| *s.borrow_mut() += duration);
}

// Thread-local counter for dropped metrics (when channel disconnects)
thread_local! {
    static DROPPED_METRICS: RefCell<u64> = const { RefCell::new(0) };
}

/// Get the count of dropped metrics for the current thread
pub fn get_dropped_metrics_count() -> u64 {
    DROPPED_METRICS.with(|c| *c.borrow())
}

/// Reset dropped metrics counter (call at start of test)
pub fn reset_dropped_metrics_count() {
    DROPPED_METRICS.with(|c| *c.borrow_mut() = 0);
}

/// Send a metric to the aggregator, logging if the channel is disconnected
#[inline]
pub fn send_metric(tx: &Sender<Metric>, metric: Metric) {
    if tx.send(metric).is_err() {
        DROPPED_METRICS.with(|c| *c.borrow_mut() += 1);
    }
}

fn print(tx: Sender<Metric>, msg: String) {
    // Only print to stdout when TUI is not active
    if !TUI_ACTIVE.load(Ordering::Relaxed) {
        println!("{}", msg);
    }
    // Send to metrics channel for aggregation
    send_metric(&tx, Metric::Log { message: msg });
}

/// Format a JS value for console output
fn format_value(v: &Value) -> String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if let Some(s) = v.as_string() {
        s.to_string().unwrap_or_default()
    } else if let Some(n) = v.as_int() {
        n.to_string()
    } else if let Some(n) = v.as_float() {
        n.to_string()
    } else if let Some(b) = v.as_bool() {
        b.to_string()
    } else if v.is_object() || v.is_array() {
        // Try to get string representation for objects/arrays
        if let Some(s) = v.as_string() {
            s.to_string().unwrap_or_else(|_| "[object]".to_string())
        } else {
            // Fallback
            "[object]".to_string()
        }
    } else {
        "[value]".to_string()
    }
}

/// Log with level filtering and optional scenario filtering
fn log_with_level(tx: &Sender<Metric>, level: LogLevel, args: Vec<Value>) {
    LOG_LEVEL.with(|current| {
        if level >= *current.borrow() {
            // Check log filter (e.g., "scenario=login")
            let filtered = LOG_FILTER.with(|filter| {
                if let Some(ref f) = *filter.borrow() {
                    if let Some(scenario_name) = f.strip_prefix("scenario=") {
                        return CURRENT_SCENARIO.with(|s| {
                            if let Some(ref current_scenario) = *s.borrow() {
                                current_scenario != scenario_name
                            } else {
                                true // No scenario set, filter it out
                            }
                        });
                    }
                }
                false // No filter, don't filter anything
            });

            if filtered {
                return;
            }

            // Format args
            let msg = args.iter().map(format_value).collect::<Vec<_>>().join(" ");

            let prefix = match level {
                LogLevel::Debug => "[DEBUG]",
                LogLevel::Info => "",
                LogLevel::Warn => "[WARN]",
                LogLevel::Error => "[ERROR]",
            };

            let full_msg = if prefix.is_empty() {
                msg
            } else {
                format!("{} {}", prefix, msg)
            };

            if !TUI_ACTIVE.load(Ordering::Relaxed) {
                println!("{}", full_msg);
            }
            send_metric(tx, Metric::Log { message: full_msg });
        }
    });
}

/// Format table data for console.table
fn format_table(args: Vec<Value>) -> String {
    if args.is_empty() {
        return String::new();
    }

    let first = &args[0];
    if !first.is_array() {
        return format_value(first);
    }

    // Try to format as table
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut headers: Vec<String> = Vec::new();

    if let Some(arr) = first.as_array() {
        for i in 0..arr.len() {
            if let Ok(item) = arr.get::<Value>(i) {
                if let Some(obj) = item.as_object() {
                    if headers.is_empty() {
                        // Extract headers from first object
                        headers = obj.keys::<String>().flatten().collect();
                    }
                    let mut row = Vec::new();
                    for h in &headers {
                        if let Ok(val) = obj.get::<_, Value>(h.as_str()) {
                            row.push(format_value(&val));
                        } else {
                            row.push(String::new());
                        }
                    }
                    rows.push(row);
                } else {
                    // Not an object, just format the value
                    rows.push(vec![format_value(&item)]);
                }
            }
        }
    }

    if headers.is_empty() && rows.is_empty() {
        return format_value(first);
    }

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Build table output
    let mut output = String::new();

    // Header row
    if !headers.is_empty() {
        let header_line: Vec<String> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| format!("{:width$}", h, width = widths.get(i).copied().unwrap_or(0)))
            .collect();
        output.push_str(&header_line.join(" | "));
        output.push('\n');

        // Separator
        let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        output.push_str(&sep.join("-+-"));
        output.push('\n');
    }

    // Data rows
    for row in &rows {
        let row_line: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                format!(
                    "{:width$}",
                    cell,
                    width = widths.get(i).copied().unwrap_or(0)
                )
            })
            .collect();
        output.push_str(&row_line.join(" | "));
        output.push('\n');
    }

    output
}

/// Register the console object with log, info, warn, error, debug, table methods
fn register_console(ctx: &Ctx, tx: Sender<Metric>) -> Result<()> {
    let console = Object::new(ctx.clone())?;

    // console.log - info level
    let tx_log = tx.clone();
    console.set(
        "log",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx; // suppress unused warning
                log_with_level(&tx_log, LogLevel::Info, args.0);
            },
        ),
    )?;

    // console.info - info level (same as log)
    let tx_info = tx.clone();
    console.set(
        "info",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx;
                log_with_level(&tx_info, LogLevel::Info, args.0);
            },
        ),
    )?;

    // console.warn - warning level
    let tx_warn = tx.clone();
    console.set(
        "warn",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx;
                log_with_level(&tx_warn, LogLevel::Warn, args.0);
            },
        ),
    )?;

    // console.error - error level
    let tx_error = tx.clone();
    console.set(
        "error",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx;
                log_with_level(&tx_error, LogLevel::Error, args.0);
            },
        ),
    )?;

    // console.debug - debug level (only shown with --log-level debug)
    let tx_debug = tx.clone();
    console.set(
        "debug",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx;
                log_with_level(&tx_debug, LogLevel::Debug, args.0);
            },
        ),
    )?;

    // console.table - tabular format
    let tx_table = tx;
    console.set(
        "table",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx, args: rquickjs::function::Rest<Value>| {
                let _ = ctx;
                LOG_LEVEL.with(|current| {
                    if LogLevel::Info >= *current.borrow() {
                        let table_output = format_table(args.0);
                        if !TUI_ACTIVE.load(Ordering::Relaxed) {
                            println!("{}", table_output);
                        }
                        send_metric(
                            &tx_table,
                            Metric::Log {
                                message: table_output,
                            },
                        );
                    }
                });
            },
        ),
    )?;

    ctx.globals().set("console", console)?;
    Ok(())
}

pub fn register_env(ctx: &Ctx) -> Result<()> {
    let env_obj = rquickjs::Object::new(ctx.clone())?;
    for (key, value) in std::env::vars() {
        env_obj.set(key, value)?;
    }
    ctx.globals().set("__ENV", env_obj)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn register_globals_sync<'js>(
    ctx: &Ctx<'js>,
    tx: Sender<Metric>,
    client: HttpClient,
    shared_data: data::SharedData,
    worker_id: usize,
    aggregator: SharedAggregator,
    runtime: Arc<Runtime>,
    jitter: Option<String>,
    drop: Option<f64>,
    response_sink: bool,
    io_bridge: Option<Arc<IoBridge>>,
) -> Result<()> {
    let globals = ctx.globals();
    let tx_print = tx.clone();
    globals.set(
        "print",
        Function::new(ctx.clone(), move |msg: String| {
            print(tx_print.clone(), msg);
        }),
    )?;

    globals.set(
        "sleep",
        Function::new(ctx.clone(), move |secs: f64| {
            let dur = Duration::from_secs_f64(secs);
            track_sleep(dur);
            std::thread::sleep(dur);
        }),
    )?;

    globals.set(
        "sleepRandom",
        Function::new(ctx.clone(), move |min: f64, max: f64| {
            let duration = if min >= max {
                min
            } else {
                min + rand::thread_rng().gen::<f64>() * (max - min)
            };
            let dur = Duration::from_secs_f64(duration);
            track_sleep(dur);
            std::thread::sleep(dur);
        }),
    )?;

    globals.set("__WORKER_ID", worker_id)?;

    // Create empty __WORKER_STATE object that persists across iterations
    // __VU_STATE is kept as alias for backwards compatibility
    let worker_state = Object::new(ctx.clone())?;
    globals.set("__WORKER_STATE", worker_state.clone())?;
    globals.set("__VU_STATE", worker_state)?;

    // Register internal modules
    // When io_bridge is present (no_pool mode), use http_sync with IoBridge routing
    // for per-request connection timing. Otherwise use standard Tokio HTTP path.
    if let Some(bridge) = io_bridge {
        http_sync::register_sync_http(ctx, tx.clone(), response_sink, Some(bridge))?;
    } else {
        http::register_sync(
            ctx,
            tx.clone(),
            client,
            runtime,
            jitter,
            drop,
            response_sink,
        )?;
    }
    check::register_sync(ctx, tx.clone())?;
    group::register_sync(ctx)?;
    file::register_sync(ctx)?;
    data::register_sync(ctx, shared_data)?;
    utils::register_sync(ctx, worker_id)?;
    ws::register_sync(ctx, tx.clone())?;
    grpc::register_sync(ctx, tx.clone())?;
    stats::register_sync(ctx, aggregator)?;
    encoding::register_sync(ctx)?;
    crypto::register_sync(ctx)?;
    mqtt::register_sync(ctx, tx.clone())?;
    amqp::register_sync(ctx, tx.clone())?;
    test::register_sync(ctx)?;
    browser::register_sync(ctx, tx.clone())?;
    sse::register_sync(ctx, tx.clone())?;
    metrics::register_sync(ctx, tx.clone())?;
    register_console(ctx, tx)?;
    register_env(ctx)?;

    Ok(())
}

/// Fast sync registration using ureq - bypasses Tokio for lower latency
/// Use this for May green thread workers
#[allow(clippy::too_many_arguments)]
pub fn register_globals_sync_fast<'js>(
    ctx: &Ctx<'js>,
    tx: Sender<Metric>,
    shared_data: data::SharedData,
    worker_id: usize,
    aggregator: SharedAggregator,
    response_sink: bool,
    io_bridge: Option<Arc<IoBridge>>,
) -> Result<()> {
    let globals = ctx.globals();
    let tx_print = tx.clone();
    globals.set(
        "print",
        Function::new(ctx.clone(), move |msg: String| {
            print(tx_print.clone(), msg);
        }),
    )?;

    globals.set(
        "sleep",
        Function::new(ctx.clone(), move |secs: f64| {
            // Use may::coroutine::sleep for green thread compatibility
            let dur = Duration::from_secs_f64(secs);
            track_sleep(dur);
            may::coroutine::sleep(dur);
        }),
    )?;

    globals.set(
        "sleepRandom",
        Function::new(ctx.clone(), move |min: f64, max: f64| {
            let duration = if min >= max {
                min
            } else {
                min + rand::thread_rng().gen::<f64>() * (max - min)
            };
            // Use may::coroutine::sleep for green thread compatibility
            let dur = Duration::from_secs_f64(duration);
            track_sleep(dur);
            may::coroutine::sleep(dur);
        }),
    )?;

    globals.set("__WORKER_ID", worker_id)?;

    // Create empty __WORKER_STATE object that persists across iterations
    // __VU_STATE is kept as alias for backwards compatibility
    let worker_state = Object::new(ctx.clone())?;
    globals.set("__WORKER_STATE", worker_state.clone())?;
    globals.set("__VU_STATE", worker_state)?;

    // Use sync HTTP (ureq) - no Tokio overhead
    // When io_bridge is present, routes through Hyper with pool disabled for connection timing
    http_sync::register_sync_http(ctx, tx.clone(), response_sink, io_bridge)?;
    check::register_sync(ctx, tx.clone())?;
    group::register_sync(ctx)?;
    file::register_sync(ctx)?;
    data::register_sync(ctx, shared_data)?;
    utils::register_sync(ctx, worker_id)?;
    // Note: ws, grpc, mqtt, amqp, sse still need async runtime - skip for fast path
    stats::register_sync(ctx, aggregator)?;
    encoding::register_sync(ctx)?;
    crypto::register_sync(ctx)?;
    test::register_sync(ctx)?;
    browser::register_sync(ctx, tx.clone())?;
    metrics::register_sync(ctx, tx.clone())?;
    register_console(ctx, tx)?;
    register_env(ctx)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dropped_metrics_counter() {
        // Reset counter
        reset_dropped_metrics_count();
        assert_eq!(get_dropped_metrics_count(), 0);

        // Create a channel and drop the receiver to simulate disconnection
        let (tx, rx) = crossbeam_channel::unbounded::<Metric>();
        drop(rx);

        // Send should fail since receiver is dropped
        send_metric(
            &tx,
            Metric::Log {
                message: "test".to_string(),
            },
        );

        // Counter should be incremented
        assert_eq!(get_dropped_metrics_count(), 1);

        // Send again
        send_metric(
            &tx,
            Metric::Log {
                message: "test2".to_string(),
            },
        );
        assert_eq!(get_dropped_metrics_count(), 2);

        // Reset should clear the counter
        reset_dropped_metrics_count();
        assert_eq!(get_dropped_metrics_count(), 0);
    }

    #[test]
    fn test_send_metric_success() {
        // Reset counter
        reset_dropped_metrics_count();

        // Create a channel with active receiver
        let (tx, rx) = crossbeam_channel::unbounded::<Metric>();

        // Send should succeed
        send_metric(
            &tx,
            Metric::Log {
                message: "test".to_string(),
            },
        );

        // Counter should remain 0
        assert_eq!(get_dropped_metrics_count(), 0);

        // Verify the message was received
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_log_filter_set_and_scenario() {
        // Set a log filter
        set_log_filter("scenario=login".to_string());
        LOG_FILTER.with(|f| {
            assert_eq!(*f.borrow(), Some("scenario=login".to_string()));
        });

        // Set current scenario
        set_current_scenario("login".to_string());
        CURRENT_SCENARIO.with(|s| {
            assert_eq!(*s.borrow(), Some("login".to_string()));
        });

        // Clean up
        LOG_FILTER.with(|f| *f.borrow_mut() = None);
        CURRENT_SCENARIO.with(|s| *s.borrow_mut() = None);
    }

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }

    #[test]
    fn test_set_and_get_log_level() {
        let original = get_log_level();
        set_log_level(LogLevel::Debug);
        assert_eq!(get_log_level(), LogLevel::Debug);
        set_log_level(LogLevel::Error);
        assert_eq!(get_log_level(), LogLevel::Error);
        // Restore
        set_log_level(original);
    }
}
