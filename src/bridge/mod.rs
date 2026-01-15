mod http;
mod http_sync;
mod check;
mod group;
mod file;
mod utils;
mod ws;
mod grpc;
mod stats;
mod encoding;
mod crypto;
mod mqtt;
mod amqp;
mod test;
mod browser;
mod sse;
pub mod data; // Make public so Engine can see SharedData type
pub mod metrics;
pub mod replay;

use rquickjs::{Ctx, Function, Result};
use std::time::Duration;
use crossbeam_channel::Sender;
use crate::stats::{Metric, SharedAggregator};
use crate::engine::http_client::HttpClient;
use std::sync::Arc;
use tokio::runtime::Runtime;

fn print(tx: Sender<Metric>, msg: String) {
    // Print to stdout for user visibility
    println!("{}", msg);
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
pub fn register_globals_sync<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>, client: HttpClient, shared_data: data::SharedData, worker_id: usize, aggregator: SharedAggregator, runtime: Arc<Runtime>, jitter: Option<String>, drop: Option<f64>, response_sink: bool) -> Result<()> {
    let globals = ctx.globals();
    let tx_print = tx.clone();
    globals.set("print", Function::new(ctx.clone(), move |msg: String| {
        print(tx_print.clone(), msg);
    }))?;

    globals.set("sleep", Function::new(ctx.clone(), move |secs: f64| {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }))?;

    globals.set("__WORKER_ID", worker_id)?;

    // Register internal modules
    http::register_sync(ctx, tx.clone(), client, runtime, jitter, drop, response_sink)?;
    check::register_sync(ctx, tx.clone())?;
    group::register_sync(ctx)?;
    file::register_sync(ctx)?;
    data::register_sync(ctx, shared_data)?;
    utils::register_sync(ctx)?;
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
pub fn register_globals_sync_fast<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>, shared_data: data::SharedData, worker_id: usize, aggregator: SharedAggregator, response_sink: bool) -> Result<()> {
    let globals = ctx.globals();
    let tx_print = tx.clone();
    globals.set("print", Function::new(ctx.clone(), move |msg: String| {
        print(tx_print.clone(), msg);
    }))?;

    globals.set("sleep", Function::new(ctx.clone(), move |secs: f64| {
        // Use may::coroutine::sleep for green thread compatibility
        may::coroutine::sleep(Duration::from_secs_f64(secs));
    }))?;

    globals.set("__WORKER_ID", worker_id)?;

    // Use sync HTTP (ureq) - no Tokio overhead
    http_sync::register_sync_http(ctx, tx.clone(), response_sink)?;
    check::register_sync(ctx, tx.clone())?;
    group::register_sync(ctx)?;
    file::register_sync(ctx)?;
    data::register_sync(ctx, shared_data)?;
    utils::register_sync(ctx)?;
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
