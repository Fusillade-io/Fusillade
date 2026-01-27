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

use crate::engine::http_client::HttpClient;
use crate::stats::{Metric, SharedAggregator};
use crossbeam_channel::Sender;
use rand::Rng;
use rquickjs::{Ctx, Function, Result};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

/// When true, suppress stdout output (TUI is active on the alternate screen)
pub static TUI_ACTIVE: AtomicBool = AtomicBool::new(false);

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

fn print(tx: Sender<Metric>, msg: String) {
    // Only print to stdout when TUI is not active
    if !TUI_ACTIVE.load(Ordering::Relaxed) {
        println!("{}", msg);
    }
    // Send to metrics channel for aggregation
    let _ = tx.send(Metric::Log { message: msg });
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

    // Register internal modules
    http::register_sync(
        ctx,
        tx.clone(),
        client,
        runtime,
        jitter,
        drop,
        response_sink,
    )?;
    check::register_sync(ctx, tx.clone())?;
    group::register_sync(ctx)?;
    file::register_sync(ctx)?;
    data::register_sync(ctx, shared_data)?;
    utils::register_sync(ctx, worker_id)?;
    ws::register_sync(ctx)?;
    grpc::register_sync(ctx)?;
    stats::register_sync(ctx, aggregator)?;
    encoding::register_sync(ctx)?;
    crypto::register_sync(ctx)?;
    mqtt::register_sync(ctx)?;
    amqp::register_sync(ctx)?;
    test::register_sync(ctx)?;
    browser::register_sync(ctx)?;
    sse::register_sync(ctx)?;
    metrics::register_sync(ctx, tx)?;
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

    // Use sync HTTP (ureq) - no Tokio overhead
    http_sync::register_sync_http(ctx, tx.clone(), response_sink)?;
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
    browser::register_sync(ctx)?;
    metrics::register_sync(ctx, tx)?;
    register_env(ctx)?;

    Ok(())
}
