use crate::cli::config::{Config, ExecutorType};
use crate::engine::distributed::MetricBatch;
use crate::engine::http_client::HttpClient;
use crate::engine::io_bridge::IoBridge;
use crate::stats::{Metric, ShardedAggregator, SharedAggregator, StatsAggregator};
use anyhow::Result;
use crossbeam_channel::{self, Receiver, Sender};
use http::Method;
#[allow(unused_imports)]
use may::coroutine;
use rquickjs::{
    loader::{FileResolver, ScriptLoader},
    CatchResultExt, CaughtError, Context, Ctx, Function, Module, Object, Runtime, Value,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::thread::JoinHandle;
use tokio::time::{Duration, Instant};

/// Format a CaughtError with detailed message and stack trace (including line numbers)
fn format_js_error(error: &CaughtError) -> String {
    match error {
        CaughtError::Exception(ex) => {
            let message = ex.message().unwrap_or_else(|| "Unknown error".to_string());
            if let Some(stack) = ex.stack() {
                format!("{}\n{}", message, stack)
            } else {
                message
            }
        }
        CaughtError::Value(val) => {
            format!("Exception: {:?}", val)
        }
        CaughtError::Error(e) => e.to_string(),
    }
}

/// Validate a scenario name for use in metrics and logging
fn validate_scenario_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("Scenario name cannot be empty"));
    }
    if name.len() > 64 {
        return Err(anyhow::anyhow!(
            "Scenario name '{}' is too long (max 64 characters)",
            name
        ));
    }
    // Check for characters that could cause issues in metrics or file paths
    for c in name.chars() {
        if !c.is_alphanumeric() && c != '_' && c != '-' && c != '.' {
            return Err(anyhow::anyhow!(
                "Scenario name '{}' contains invalid character '{}'. Use only alphanumeric, underscore, hyphen, or dot.",
                name, c
            ));
        }
    }
    Ok(())
}

fn parse_duration_str(s: &str) -> Option<Duration> {
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
    } else {
        s.parse::<u64>().ok().map(Duration::from_millis)
    }
}

pub struct Engine {
    shared_data: crate::bridge::data::SharedData,
}

pub mod control;
pub mod distributed;
pub mod http_client;
pub mod io_bridge;
pub mod memory;

impl Engine {
    pub fn new() -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            shared_data: Arc::new(std::sync::RwLock::new(std::collections::HashMap::new())),
        })
    }

    fn create_runtime() -> Result<(Runtime, Context)> {
        let runtime = Runtime::new()?;
        let resolver = FileResolver::default()
            .with_path("./")
            .with_path("./scenarios")
            .with_path("./support");
        let loader = ScriptLoader::default();
        runtime.set_loader(resolver, loader);
        let context = Context::full(&runtime)?;
        Ok((runtime, context))
    }

    pub fn extract_config(
        &self,
        script_path: PathBuf,
        script_content: String,
    ) -> Result<Option<Config>> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let dummy_shared_data = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let dummy_aggregator = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));
        let tokio_rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        );
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        let config = context.with(|ctx| {
            crate::bridge::register_globals_sync(
                &ctx,
                tx,
                client,
                dummy_shared_data,
                0,
                dummy_aggregator,
                tokio_rt,
                None,
                None,
                false,
            )?;
            let module_name = script_path.to_string_lossy().to_string();
            let module = Module::declare(ctx.clone(), module_name, script_content)?;
            let (module, _) = module.eval()?;
            if let Ok(options) = module.get::<_, Value>("options") {
                if options.is_object() {
                    let json_str = json_stringify(ctx.clone(), options)?;
                    let mut config_val: serde_json::Value = serde_json::from_str(&json_str)?;
                    if let Some(obj) = config_val.as_object_mut() {
                        if let Some(s) = obj.remove("stages") {
                            obj.insert("schedule".to_string(), s);
                        }
                        if let Some(t) = obj.remove("thresholds") {
                            obj.insert("criteria".to_string(), t);
                        }
                        if let Some(e) = obj.remove("executor") {
                            obj.insert("executor".to_string(), e);
                        }
                        if let Some(r) = obj.remove("rate") {
                            obj.insert("rate".to_string(), r);
                        }
                        if let Some(tu) = obj.remove("timeUnit") {
                            obj.insert("time_unit".to_string(), tu);
                        }
                        if let Some(mid) = obj.remove("minIterationDuration") {
                            obj.insert("min_iteration_duration".to_string(), mid);
                        }
                        if let Some(rs) = obj.remove("responseSink") {
                            obj.insert("response_sink".to_string(), rs);
                        }

                        // Handle scenarios with alias key mapping
                        if let Some(scenarios) = obj.get_mut("scenarios") {
                            if let Some(scenarios_obj) = scenarios.as_object_mut() {
                                for (_name, scenario) in scenarios_obj.iter_mut() {
                                    if let Some(scenario_obj) = scenario.as_object_mut() {
                                        // Map alias keys to canonical keys
                                        if let Some(vus) = scenario_obj.remove("vus") {
                                            scenario_obj.insert("workers".to_string(), vus);
                                        }
                                        if let Some(stages) = scenario_obj.remove("stages") {
                                            scenario_obj.insert("schedule".to_string(), stages);
                                        }
                                        if let Some(tu) = scenario_obj.remove("timeUnit") {
                                            scenario_obj.insert("time_unit".to_string(), tu);
                                        }
                                        if let Some(rs) = scenario_obj.remove("responseSink") {
                                            scenario_obj.insert("response_sink".to_string(), rs);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let config: Config = serde_json::from_value(config_val)?;
                    return Ok(Some(config));
                }
            }
            Ok::<Option<Config>, anyhow::Error>(None)
        });
        // Cleanup: run GC and then forget the runtime to prevent cleanup assertion failures
        // This leaks memory but prevents crashes during runtime shutdown
        // (rquickjs can fail to clean up all objects due to closures stored in globals)
        runtime.run_gc();
        drop(context);
        std::mem::forget(runtime);
        config
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_load_test(
        self: Arc<Self>,
        script_path: PathBuf,
        script_content: String,
        config: Config,
        json_output: bool,
        export_json: Option<PathBuf>,
        export_html: Option<PathBuf>,
        controller_metrics_url: Option<String>,
        controller_metrics_auth: Option<String>,
        control_rx: Option<std::sync::mpsc::Receiver<control::ControlCommand>>,
        shared_aggregator: Option<Arc<ShardedAggregator>>,
        shared_control_state: Option<Arc<control::ControlState>>,
    ) -> Result<crate::stats::ReportStats> {
        if let Some(url) = &config.warmup {
            self.warmup(url);
        }

        let config = Arc::new(config);
        let shared_data = self.shared_data.clone();
        let script_content = script_content.clone();
        let script_path = script_path.clone();

        let min_iter_duration = if let Some(s) = &config.min_iteration_duration {
            parse_duration(s).ok()
        } else {
            None
        };

        // Initialize control state early (use shared if provided by TUI)
        let control_state = shared_control_state
            .unwrap_or_else(|| Arc::new(control::ControlState::new(config.workers.unwrap_or(1))));

        // 1. Run Setup (same as before)
        let setup_data = self
            .run_setup(
                &script_path,
                &script_content,
                Arc::new(std::sync::RwLock::new(StatsAggregator::new())),
            )
            .unwrap_or_else(|e| {
                eprintln!("Setup failed: {}", e);
                None
            });

        let setup_data = Arc::new(setup_data);

        // Pre-validate script: compile and check for exec function before spawning workers
        {
            let (pre_rt, pre_ctx) = Self::create_runtime()?;
            let validation_err = pre_ctx.with(|ctx| {
                let (pre_tx, _) = crossbeam_channel::unbounded();
                let pre_agg = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));
                let pre_tokio = Arc::new(
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap(),
                );
                let pre_client = {
                    let _guard = pre_tokio.enter();
                    HttpClient::new()
                };
                let pre_shared = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
                crate::bridge::register_globals_sync(
                    &ctx, pre_tx, pre_client, pre_shared, 0, pre_agg, pre_tokio, None, None, false,
                )
                .ok();

                let module = match rquickjs::Module::declare(
                    ctx.clone(),
                    script_path.to_string_lossy().to_string(),
                    script_content.clone(),
                )
                .catch(&ctx)
                {
                    Ok(m) => m,
                    Err(e) => {
                        return Some(format!(
                            "Script compilation failed:\n{}",
                            format_js_error(&e)
                        ));
                    }
                };
                let (module, _) = match module.eval().catch(&ctx) {
                    Ok(m) => m,
                    Err(e) => {
                        return Some(format!("Script evaluation failed:\n{}", format_js_error(&e)));
                    }
                };

                // Check for default function (or scenario exec functions)
                let has_default = module.get::<_, rquickjs::Function>("default").is_ok();
                let has_scenarios = config.scenarios.is_some();
                if !has_default && !has_scenarios {
                    return Some(
                        "Script error: 'default' function not found. Did you forget to export default function?".to_string(),
                    );
                }
                if let Some(ref scenarios) = config.scenarios {
                    for (name, sc) in scenarios {
                        let exec_fn = sc.exec.clone().unwrap_or_else(|| "default".to_string());
                        if module.get::<_, rquickjs::Function>(&exec_fn).is_err()
                            && module.get::<_, rquickjs::Function>("default").is_err()
                        {
                            return Some(format!(
                                "Scenario '{}': function '{}' not found and no 'default' fallback",
                                name, exec_fn
                            ));
                        }
                    }
                }
                None
            });
            pre_rt.run_gc();
            drop(pre_ctx);
            std::mem::forget(pre_rt);

            if let Some(err_msg) = validation_err {
                return Err(anyhow::anyhow!("{}", err_msg));
            }
        }

        let handle = std::thread::spawn(move || {
            // Calculate total workers for scaling decisions
            let total_workers = if let Some(ref scenarios) = config.scenarios {
                scenarios
                    .values()
                    .map(|s| s.workers.unwrap_or(1))
                    .sum::<usize>()
            } else {
                config.workers.unwrap_or(1)
            };

            // Dynamic shard count: target ~100 workers per shard for optimal contention
            // At 10k workers: 100 shards = 100 workers/shard (vs old 625 workers/shard)
            let num_shards = (total_workers / 100).clamp(16, 256);
            let sharded_aggregator = shared_aggregator.unwrap_or_else(|| {
                if config.no_endpoint_tracking.unwrap_or(false) {
                    Arc::new(ShardedAggregator::new_no_endpoint_tracking(num_shards))
                } else {
                    Arc::new(ShardedAggregator::new(num_shards))
                }
            });

            // Use crossbeam bounded channel for backpressure at extreme load
            // Scale buffer with workers, with higher limits for extreme concurrency
            let channel_size = if total_workers > 30000 {
                (total_workers * 5).clamp(50_000, 250_000)
            } else {
                (total_workers * 10).clamp(20_000, 100_000)
            };
            let (tx, rx): (Sender<Metric>, Receiver<Metric>) =
                crossbeam_channel::bounded(channel_size);

            let agg_handle = sharded_aggregator.clone();
            let metrics_url = controller_metrics_url.clone();
            let metrics_auth = controller_metrics_auth.clone();

            // Spawn multiple aggregator consumer threads for parallel metric processing
            // This prevents the single-consumer bottleneck at high throughput
            let num_aggregators = num_cpus::get().max(2) / 2;
            for agg_id in 0..num_aggregators {
                let rx = rx.clone();
                let agg_handle = agg_handle.clone();
                let metrics_url = metrics_url.clone();

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let client = if metrics_url.is_some() {
                        let _guard = rt.enter();
                        Some(HttpClient::new())
                    } else {
                        None
                    };

                    let mut batch = Vec::new();
                    let mut last_send = Instant::now();
                    let mut local_counter: usize = agg_id;

                    while let Ok(metric) = rx.recv() {
                        // Distribute metrics across shards using worker-local counter
                        agg_handle.add(local_counter % num_shards, metric.clone());
                        local_counter = local_counter.wrapping_add(num_aggregators);

                        // Only first aggregator handles remote reporting to avoid duplicates
                        if agg_id == 0 {
                            if let (Some(url), Some(client)) = (&metrics_url, &client) {
                                batch.push(metric);
                                if last_send.elapsed() >= Duration::from_secs(1)
                                    || batch.len() >= 100
                                {
                                    let payload = MetricBatch {
                                        worker_id: 0,
                                        metrics: std::mem::take(&mut batch),
                                    };
                                    // Fire and forget reporting
                                    let body = serde_json::to_string(&payload).unwrap_or_default();
                                    if let Ok(req) = http::Request::builder()
                                        .method(Method::POST)
                                        .uri(format!("{}/metrics", url))
                                        .body(body)
                                    {
                                        let _ = rt.block_on(client.request(req, false));
                                    }
                                    last_send = Instant::now();
                                }
                            }
                        }
                    }
                });
            }

            // Drop our copy of rx so aggregator threads can detect channel close
            drop(rx);

            // Keep a reference to the old-style aggregator for compatibility with existing code
            let aggregator = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));

            // Shared resources for all workers (memory optimization)
            let base_parallelism = std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(8);

            // Scale Tokio threads for high concurrency
            let tokio_threads = if total_workers > 5000 {
                (total_workers / 75).clamp(base_parallelism * 2, 128)
            } else if total_workers > 1000 {
                (total_workers / 50).clamp(base_parallelism, 64)
            } else {
                base_parallelism.max(8)
            };

            let shared_tokio_rt = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(tokio_threads)
                    .enable_all()
                    .build()
                    .expect("Failed to create shared Tokio runtime"),
            );
            // Scale connection pool with workers: target ~1 idle connection per 5 workers
            let pool_size = (total_workers / 5).clamp(500, 2000);
            let shared_http_client = {
                let _guard = shared_tokio_rt.enter();
                HttpClient::with_pool_and_workers(pool_size, total_workers)
            };

            // Configure may green thread scheduler with fixed stack size
            // 32KB works for 50K+ workers and complex scripts. Override via options.stack_size if needed.
            let may_stack_size = 32 * 1024;
            // Scale May workers proportionally to total workers
            // Each May thread can block on HTTP I/O, so we need many threads for high concurrency
            // Formula: 1 May worker per ~200 Fusillade workers, minimum of CPU count, max 128
            let may_workers = (total_workers / 200).clamp(num_cpus::get(), 128);
            may::config()
                .set_workers(may_workers)
                .set_stack_size(may_stack_size);

            // Create I/O bridge for may coroutines to access Tokio HTTP
            let num_io_workers = tokio_threads.min(32); // I/O workers match Tokio threads
            let _io_bridge = Arc::new(IoBridge::new(
                shared_tokio_rt.clone(),
                Arc::new(shared_http_client.clone()),
                num_io_workers,
            ));

            let start_time = Instant::now();
            let mut active_scenario_handles: Vec<JoinHandle<()>> = Vec::new(); // For multi-scenario
            let mut active_legacy_workers: Vec<(may::coroutine::JoinHandle<()>, Arc<AtomicBool>)> =
                Vec::new(); // For legacy (green threads)
            let mut dynamic_target_legacy: Option<usize> = None; // For legacy ramp
            let schedule_legacy: Vec<(Duration, usize)>;
            let duration_legacy: Duration;
            let mut next_worker_id = 1;

            // Memory-safe mode: Set up monitoring flags
            let memory_throttle = Arc::new(AtomicBool::new(false));
            let memory_critical = Arc::new(AtomicBool::new(false));
            let _memory_monitor = if config.memory_safe.unwrap_or(false) {
                let throttle_flag = memory_throttle.clone();
                let critical_flag = memory_critical.clone();
                Some(memory::MemoryMonitor::start(
                    Duration::from_secs(2),
                    move |info| {
                        // Warning threshold (85%): pause new worker spawning
                        eprintln!("[Memory] Warning: {:.1}% used ({} available). Throttling worker spawns.",
                            info.usage_percent() * 100.0,
                            memory::format_bytes(info.available_bytes));
                        throttle_flag.store(true, Ordering::Relaxed);
                    },
                    move |info| {
                        // Critical threshold (95%): signal for worker reduction
                        eprintln!(
                            "[Memory] CRITICAL: {:.1}% used ({} available). Stopping new workers.",
                            info.usage_percent() * 100.0,
                            memory::format_bytes(info.available_bytes)
                        );
                        critical_flag.store(true, Ordering::Relaxed);
                    },
                ))
            } else {
                None
            };

            // Check if we have multiple scenarios
            let is_multi_scenario = config.scenarios.is_some();

            if let Some(ref scenarios) = config.scenarios {
                // Validate all scenario names upfront
                for name in scenarios.keys() {
                    validate_scenario_name(name)?;
                }

                // Multi-scenario mode: spawn a worker pool per scenario
                for (scenario_name, scenario_config) in scenarios.iter() {
                    let scenario_name = scenario_name.clone();
                    let scenario_config = scenario_config.clone();
                    let tx = tx.clone();
                    let aggregator = aggregator.clone();
                    let shared_data = shared_data.clone();
                    let script_content = script_content.clone();
                    let script_path = script_path.clone();
                    let setup_data = setup_data.clone();
                    let min_iter_duration = min_iter_duration;
                    let jitter = config.jitter.clone();
                    let drop_rate = config.drop;
                    let shared_tokio_rt = shared_tokio_rt.clone();
                    let shared_http_client = shared_http_client.clone();
                    let control_state = control_state.clone();
                    let graceful_stop = config.stop.clone();

                    let h = std::thread::spawn(move || {
                        // Handle startTime delay
                        if let Some(ref st) = scenario_config.start_time {
                            if let Some(delay) = parse_duration_str(st) {
                                std::thread::sleep(delay);
                            }
                        }

                        let exec_fn = scenario_config
                            .exec
                            .clone()
                            .unwrap_or_else(|| "default".to_string());
                        let duration = scenario_config.duration.as_deref().unwrap_or("10s");
                        let scenario_duration =
                            parse_duration(duration).unwrap_or(Duration::from_secs(10));
                        let workers = scenario_config.workers.unwrap_or(1);
                        let max_iterations = scenario_config.iterations;
                        let executor = scenario_config.executor.unwrap_or_default();

                        // Get rate config for arrival-rate executors
                        let rate = scenario_config.rate.unwrap_or(1);
                        let time_unit = parse_duration_str(
                            scenario_config.time_unit.as_deref().unwrap_or("1s"),
                        )
                        .unwrap_or(Duration::from_secs(1));

                        // For ramping-arrival-rate, parse schedule
                        let rate_schedule: Vec<(Duration, u64)> =
                            if let Some(ref steps) = scenario_config.schedule {
                                steps
                                    .iter()
                                    .map(|s| {
                                        let dur = parse_duration(&s.duration)
                                            .unwrap_or(Duration::from_secs(10));
                                        (dur, s.target as u64)
                                    })
                                    .collect()
                            } else {
                                vec![(scenario_duration, rate)]
                            };

                        match executor {
                            ExecutorType::ConstantArrivalRate
                            | ExecutorType::RampingArrivalRate => {
                                println!(
                                    "[Scenario: {}] Starting arrival-rate executor ({} workers, {} rate/{:?}) for {}...",
                                    scenario_name, workers, rate, time_unit, duration
                                );

                                let start_time = Instant::now();
                                let running = Arc::new(AtomicBool::new(true));
                                let dropped_iterations = Arc::new(AtomicU64::new(0));
                                let iterations_started = Arc::new(AtomicU64::new(0));
                                let iterations_completed = Arc::new(AtomicU64::new(0));

                                // Create bounded channel for iteration requests
                                // Buffer size = workers * 2 to allow some queuing but provide backpressure
                                let (iter_tx, iter_rx) =
                                    crossbeam_channel::bounded::<u64>(workers * 2);

                                // Spawn worker pool - these workers wait for iteration requests
                                let mut worker_handles: Vec<may::coroutine::JoinHandle<()>> =
                                    Vec::new();
                                for worker_id in 0..workers {
                                    let iter_rx = iter_rx.clone();
                                    let tx = tx.clone();
                                    let shared_data = shared_data.clone();
                                    let agg = aggregator.clone();
                                    let running = running.clone();
                                    let script_content = script_content.clone();
                                    let script_path = script_path.clone();
                                    let setup_data_clone = setup_data.clone();
                                    let exec_fn = exec_fn.clone();
                                    let scenario_name = scenario_name.clone();
                                    let jitter = jitter.clone();
                                    let drop_rate = drop_rate;
                                    let stack_sz =
                                        scenario_config.stack_size.unwrap_or(may_stack_size);
                                    let response_sink =
                                        scenario_config.response_sink.unwrap_or(false);
                                    let tokio_rt = shared_tokio_rt.clone();
                                    let client = shared_http_client.clone();
                                    let control_state = control_state.clone();
                                    let iterations_completed = iterations_completed.clone();

                                    // SAFETY: Stack size is configured to be sufficient for JS execution
                                    let handle = unsafe {
                                        may::coroutine::Builder::new()
                                            .stack_size(stack_sz)
                                            .spawn(move || {
                                                let (runtime, context) = Self::create_runtime().unwrap();
                                                runtime.set_memory_limit(256 * 1024);
                                                context.with(|ctx| {
                                                    crate::bridge::register_globals_sync(
                                                        &ctx, tx.clone(), client, shared_data, worker_id,
                                                        agg, tokio_rt, jitter, drop_rate, response_sink
                                                    ).unwrap();
                                                    ctx.globals().set("__SCENARIO", scenario_name.clone()).unwrap();

                                                    let module = match Module::declare(
                                                        ctx.clone(),
                                                        script_path.to_string_lossy().to_string(),
                                                        script_content
                                                    ).catch(&ctx) {
                                                        Ok(m) => m,
                                                        Err(e) => {
                                                            eprintln!("[Scenario: {}] Script error:\n{}", scenario_name, format_js_error(&e));
                                                            return;
                                                        }
                                                    };
                                                    let (module, _) = match module.eval().catch(&ctx) {
                                                        Ok(m) => m,
                                                        Err(e) => {
                                                            eprintln!("[Scenario: {}] Script error:\n{}", scenario_name, format_js_error(&e));
                                                            return;
                                                        }
                                                    };

                                                    let func: Function = match module.get(&exec_fn) {
                                                        Ok(f) => f,
                                                        Err(_) => match module.get("default") {
                                                            Ok(f) => f,
                                                            Err(_) => {
                                                                eprintln!("[Scenario: {}] No function found. Worker exiting.", scenario_name);
                                                                return;
                                                            }
                                                        }
                                                    };

                                                    let data_val = if let Some(json) = setup_data_clone.as_ref() {
                                                        json_parse(ctx.clone(), json).unwrap_or(Value::new_undefined(ctx.clone()))
                                                    } else {
                                                        Value::new_undefined(ctx.clone())
                                                    };

                                                    // Arrival-rate worker loop: wait for iteration requests from channel
                                                    while running.load(Ordering::Relaxed) {
                                                        // Check for pause
                                                        while control_state.is_paused() && running.load(Ordering::Relaxed) {
                                                            may::coroutine::sleep(Duration::from_millis(100));
                                                        }
                                                        if control_state.is_stopped() || !running.load(Ordering::Relaxed) {
                                                            break;
                                                        }

                                                        // Wait for iteration request with timeout
                                                        match iter_rx.recv_timeout(Duration::from_millis(100)) {
                                                            Ok(iteration_num) => {
                                                                crate::bridge::reset_sleep_tracker();
                                                                let _ = ctx.globals().set("__ITERATION", iteration_num);
                                                                let iter_start = Instant::now();
                                                                let call_result = func.call::<_, ()>((data_val.clone(),)).catch(&ctx);
                                                                let elapsed = iter_start.elapsed();
                                                                let sleep_time = crate::bridge::get_sleep_accumulated();
                                                                let active_time = elapsed.saturating_sub(sleep_time);

                                                                let timings = crate::stats::RequestTimings { duration: active_time, ..Default::default() };
                                                                let timings_total = crate::stats::RequestTimings { duration: elapsed, ..Default::default() };
                                                                let tags: HashMap<String, String> = control_state.get_tags();

                                                                match call_result {
                                                                    Ok(_) => {
                                                                        let _ = tx.send(Metric::Request {
                                                                            name: format!("{}::iteration", scenario_name),
                                                                            timings,
                                                                            status: 200,
                                                                            error: None,
                                                                            tags: tags.clone(),
                                                                        });
                                                                        let _ = tx.send(Metric::Request {
                                                                            name: format!("{}::iteration_total", scenario_name),
                                                                            timings: timings_total,
                                                                            status: 200,
                                                                            error: None,
                                                                            tags,
                                                                        });
                                                                    }
                                                                    Err(e) => {
                                                                        let error_msg = format_js_error(&e);
                                                                        let _ = tx.send(Metric::Request {
                                                                            name: format!("{}::iteration", scenario_name),
                                                                            timings,
                                                                            status: 0,
                                                                            error: Some(error_msg.clone()),
                                                                            tags: tags.clone(),
                                                                        });
                                                                        let _ = tx.send(Metric::Request {
                                                                            name: format!("{}::iteration_total", scenario_name),
                                                                            timings: timings_total,
                                                                            status: 0,
                                                                            error: Some(error_msg),
                                                                            tags,
                                                                        });
                                                                    }
                                                                }
                                                                iterations_completed.fetch_add(1, Ordering::Relaxed);
                                                            }
                                                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                                                                // No work available, check if we should continue
                                                                continue;
                                                            }
                                                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                                                                // Channel closed, exit
                                                                break;
                                                            }
                                                        }
                                                    }
                                                });
                                                runtime.run_gc();
                                                std::mem::drop(context);
                                                std::mem::forget(runtime);
                                            })
                                    };
                                    worker_handles
                                        .push(handle.expect("Failed to spawn arrival-rate worker"));
                                }

                                // Drop our receiver so workers can detect channel close
                                drop(iter_rx);

                                // Dispatcher: send iteration requests at the configured rate
                                let is_ramping = executor == ExecutorType::RampingArrivalRate;
                                let mut iteration_counter: u64 = 0;

                                while start_time.elapsed() < scenario_duration
                                    && running.load(Ordering::Relaxed)
                                {
                                    if control_state.is_stopped() {
                                        break;
                                    }

                                    // Calculate current rate for ramping
                                    let current_rate = if is_ramping {
                                        Self::calculate_rate(&rate_schedule, start_time.elapsed())
                                    } else {
                                        rate
                                    };

                                    // Calculate interval between iterations
                                    let interval = if current_rate > 0 {
                                        time_unit / current_rate as u32
                                    } else {
                                        Duration::from_secs(1) // Fallback if rate is 0
                                    };

                                    // Try to send iteration request
                                    match iter_tx.try_send(iteration_counter) {
                                        Ok(_) => {
                                            iterations_started.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(crossbeam_channel::TrySendError::Full(_)) => {
                                            dropped_iterations.fetch_add(1, Ordering::Relaxed);
                                        }
                                        Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                                            break;
                                        }
                                    }

                                    iteration_counter += 1;

                                    // Check iteration limit
                                    if let Some(max_iter) = max_iterations {
                                        if iteration_counter >= max_iter {
                                            break;
                                        }
                                    }

                                    // Sleep until next iteration
                                    std::thread::sleep(interval);
                                }

                                // Signal workers to stop
                                running.store(false, Ordering::Relaxed);
                                drop(iter_tx); // Close channel

                                // Report dropped iterations metric
                                let dropped = dropped_iterations.load(Ordering::Relaxed);
                                if dropped > 0 {
                                    let _ = tx.send(Metric::Counter {
                                        name: format!("{}::dropped_iterations", scenario_name),
                                        value: dropped as f64,
                                        tags: HashMap::new(),
                                    });
                                    eprintln!(
                                        "[Scenario: {}] Warning: {} iterations dropped (workers couldn't keep up)",
                                        scenario_name, dropped
                                    );
                                }

                                // Report iteration stats
                                let _ = tx.send(Metric::Counter {
                                    name: format!("{}::iterations_started", scenario_name),
                                    value: iterations_started.load(Ordering::Relaxed) as f64,
                                    tags: HashMap::new(),
                                });
                                let _ = tx.send(Metric::Counter {
                                    name: format!("{}::iterations_completed", scenario_name),
                                    value: iterations_completed.load(Ordering::Relaxed) as f64,
                                    tags: HashMap::new(),
                                });

                                // Graceful stop: wait for workers to finish
                                let grace = graceful_stop
                                    .as_deref()
                                    .and_then(parse_duration_str)
                                    .unwrap_or(Duration::from_secs(30));
                                let stop_start = Instant::now();
                                for handle in worker_handles {
                                    let remaining = grace.saturating_sub(stop_start.elapsed());
                                    if remaining > Duration::ZERO {
                                        // Join with timeout
                                        let _ = handle.join();
                                    }
                                }

                                println!("[Scenario: {}] Complete.", scenario_name);
                            }

                            // Ramping-VUs executor: dynamically scale workers based on schedule
                            ExecutorType::RampingVus => {
                                println!(
                                    "[Scenario: {}] Starting ramping-vus executor for {}...",
                                    scenario_name, duration
                                );

                                let start_time = Instant::now();
                                let mut active_workers: Vec<(
                                    may::coroutine::JoinHandle<()>,
                                    Arc<AtomicBool>,
                                )> = Vec::new();
                                let mut next_worker_id = 1usize;

                                // Parse VUs schedule from config
                                let vus_schedule: Vec<(Duration, usize)> =
                                    if let Some(ref steps) = scenario_config.schedule {
                                        steps
                                            .iter()
                                            .map(|s| {
                                                let dur = parse_duration(&s.duration)
                                                    .unwrap_or(Duration::from_secs(10));
                                                (dur, s.target)
                                            })
                                            .collect()
                                    } else {
                                        // Default: constant workers for the duration
                                        vec![(scenario_duration, workers)]
                                    };

                                // Helper closure to spawn a worker - captures all needed variables
                                let spawn_worker = |worker_id: usize,
                                                    tx: Sender<Metric>,
                                                    shared_data: crate::bridge::data::SharedData,
                                                    aggregator: SharedAggregator,
                                                    script_content: String,
                                                    script_path: PathBuf,
                                                    setup_data: Arc<Option<String>>,
                                                    exec_fn: String,
                                                    scenario_name: String,
                                                    jitter: Option<String>,
                                                    drop_rate: Option<f64>,
                                                    stack_sz: usize,
                                                    response_sink: bool,
                                                    tokio_rt: Arc<tokio::runtime::Runtime>,
                                                    client: HttpClient,
                                                    control_state: Arc<control::ControlState>,
                                                    max_iterations: Option<u64>,
                                                    min_iter_duration: Option<Duration>|
                                 -> (
                                    may::coroutine::JoinHandle<()>,
                                    Arc<AtomicBool>,
                                ) {
                                    let running = Arc::new(AtomicBool::new(true));
                                    let running_clone = running.clone();

                                    let worker_handle = unsafe {
                                        may::coroutine::Builder::new()
                                            .stack_size(stack_sz)
                                            .spawn(move || {
                                                let (runtime, context) =
                                                    Self::create_runtime().unwrap();
                                                runtime.set_memory_limit(256 * 1024);
                                                context.with(|ctx| {
                                                    crate::bridge::register_globals_sync(
                                                        &ctx,
                                                        tx.clone(),
                                                        client,
                                                        shared_data,
                                                        worker_id,
                                                        aggregator,
                                                        tokio_rt,
                                                        jitter,
                                                        drop_rate,
                                                        response_sink,
                                                    )
                                                    .unwrap();
                                                    ctx.globals()
                                                        .set("__SCENARIO", scenario_name.clone())
                                                        .unwrap();

                                                    let module = match Module::declare(
                                                        ctx.clone(),
                                                        script_path.to_string_lossy().to_string(),
                                                        script_content,
                                                    )
                                                    .catch(&ctx)
                                                    {
                                                        Ok(m) => m,
                                                        Err(e) => {
                                                            eprintln!(
                                                                "[Scenario: {}] Script error:\n{}",
                                                                scenario_name,
                                                                format_js_error(&e)
                                                            );
                                                            return;
                                                        }
                                                    };
                                                    let (module, _) =
                                                        match module.eval().catch(&ctx) {
                                                            Ok(m) => m,
                                                            Err(e) => {
                                                                eprintln!(
                                                            "[Scenario: {}] Script error:\n{}",
                                                            scenario_name,
                                                            format_js_error(&e)
                                                        );
                                                                return;
                                                            }
                                                        };

                                                    let func: Function = match module.get(&exec_fn)
                                                    {
                                                        Ok(f) => f,
                                                        Err(_) => {
                                                            eprintln!("[Scenario: {}] Function '{}' not found, trying 'default'", scenario_name, exec_fn);
                                                            match module.get("default") {
                                                                Ok(f) => f,
                                                                Err(_) => {
                                                                    eprintln!("[Scenario: {}] No 'default' function found either. Worker exiting.", scenario_name);
                                                                    return;
                                                                }
                                                            }
                                                        }
                                                    };

                                                    let data_val =
                                                        if let Some(json) = setup_data.as_ref().as_ref() {
                                                            json_parse(ctx.clone(), json)
                                                                .unwrap_or(Value::new_undefined(
                                                                    ctx.clone(),
                                                                ))
                                                        } else {
                                                            Value::new_undefined(ctx.clone())
                                                        };

                                                    let mut iteration_count: u64 = 0;
                                                    while running_clone.load(Ordering::Relaxed) {
                                                        while control_state.is_paused()
                                                            && running_clone.load(Ordering::Relaxed)
                                                        {
                                                            may::coroutine::sleep(
                                                                Duration::from_millis(100),
                                                            );
                                                        }
                                                        if control_state.is_stopped()
                                                            || !running_clone
                                                                .load(Ordering::Relaxed)
                                                        {
                                                            break;
                                                        }

                                                        crate::bridge::reset_sleep_tracker();
                                                        let _ = ctx
                                                            .globals()
                                                            .set("__ITERATION", iteration_count);
                                                        let iter_start = Instant::now();
                                                        let call_result = func
                                                            .call::<_, ()>((data_val.clone(),))
                                                            .catch(&ctx);
                                                        let elapsed = iter_start.elapsed();
                                                        let sleep_time =
                                                            crate::bridge::get_sleep_accumulated();
                                                        let active_time =
                                                            elapsed.saturating_sub(sleep_time);

                                                        let timings =
                                                            crate::stats::RequestTimings {
                                                                duration: active_time,
                                                                ..Default::default()
                                                            };
                                                        let timings_total =
                                                            crate::stats::RequestTimings {
                                                                duration: elapsed,
                                                                ..Default::default()
                                                            };

                                                        let tags: HashMap<String, String> =
                                                            control_state.get_tags();

                                                        match call_result {
                                                            Ok(_) => {
                                                                let _ = tx.send(Metric::Request {
                                                                    name: format!(
                                                                        "{}::iteration",
                                                                        scenario_name
                                                                    ),
                                                                    timings,
                                                                    status: 200,
                                                                    error: None,
                                                                    tags: tags.clone(),
                                                                });
                                                                let _ = tx.send(Metric::Request {
                                                                    name: format!(
                                                                        "{}::iteration_total",
                                                                        scenario_name
                                                                    ),
                                                                    timings: timings_total,
                                                                    status: 200,
                                                                    error: None,
                                                                    tags,
                                                                });
                                                            }
                                                            Err(e) => {
                                                                let error_msg = format_js_error(&e);
                                                                let _ = tx.send(Metric::Request {
                                                                    name: format!(
                                                                        "{}::iteration",
                                                                        scenario_name
                                                                    ),
                                                                    timings,
                                                                    status: 0,
                                                                    error: Some(error_msg.clone()),
                                                                    tags: tags.clone(),
                                                                });
                                                                let _ = tx.send(Metric::Request {
                                                                    name: format!(
                                                                        "{}::iteration_total",
                                                                        scenario_name
                                                                    ),
                                                                    timings: timings_total,
                                                                    status: 0,
                                                                    error: Some(error_msg),
                                                                    tags,
                                                                });
                                                            }
                                                        }

                                                        iteration_count += 1;
                                                        if let Some(max_iter) = max_iterations {
                                                            if iteration_count >= max_iter {
                                                                break;
                                                            }
                                                        }
                                                        if let Some(min_dur) = min_iter_duration {
                                                            if elapsed < min_dur {
                                                                may::coroutine::sleep(
                                                                    min_dur - elapsed,
                                                                );
                                                            }
                                                        }
                                                    }
                                                });
                                                runtime.run_gc();
                                                std::mem::drop(context);
                                                std::mem::forget(runtime);
                                            })
                                    };

                                    (worker_handle.expect("Failed to spawn coroutine"), running)
                                };

                                // Ramping control loop - adjust VU count every second
                                while start_time.elapsed() < scenario_duration {
                                    if control_state.is_stopped() {
                                        break;
                                    }

                                    // Calculate target VUs at current time
                                    let elapsed = start_time.elapsed();
                                    let target_vus = Self::calculate_target(&vus_schedule, elapsed);
                                    let current_vus = active_workers.len();

                                    if current_vus < target_vus {
                                        // Spawn more workers
                                        let to_spawn = target_vus - current_vus;
                                        for _ in 0..to_spawn {
                                            let (handle, running) = spawn_worker(
                                                next_worker_id,
                                                tx.clone(),
                                                shared_data.clone(),
                                                aggregator.clone(),
                                                script_content.clone(),
                                                script_path.clone(),
                                                setup_data.clone(),
                                                exec_fn.clone(),
                                                scenario_name.clone(),
                                                jitter.clone(),
                                                drop_rate,
                                                scenario_config
                                                    .stack_size
                                                    .unwrap_or(may_stack_size),
                                                scenario_config.response_sink.unwrap_or(false),
                                                shared_tokio_rt.clone(),
                                                shared_http_client.clone(),
                                                control_state.clone(),
                                                max_iterations,
                                                min_iter_duration,
                                            );
                                            active_workers.push((handle, running));
                                            next_worker_id += 1;
                                        }
                                    } else if current_vus > target_vus {
                                        // Stop excess workers (from the end)
                                        let to_stop = current_vus - target_vus;
                                        for _ in 0..to_stop {
                                            if let Some((_, running)) = active_workers.pop() {
                                                running.store(false, Ordering::Relaxed);
                                            }
                                        }
                                    }

                                    // Report current VU count
                                    let _ = tx.send(Metric::Gauge {
                                        name: format!("{}::vus", scenario_name),
                                        value: active_workers.len() as f64,
                                        tags: HashMap::new(),
                                    });

                                    // Sleep for 1 second before next adjustment
                                    std::thread::sleep(Duration::from_secs(1));
                                }

                                // Stop all remaining workers
                                for (_, r) in active_workers.iter() {
                                    r.store(false, Ordering::Relaxed);
                                }

                                // Graceful stop
                                let grace = graceful_stop
                                    .as_deref()
                                    .and_then(parse_duration_str)
                                    .unwrap_or(Duration::from_secs(30));
                                if grace > Duration::ZERO {
                                    let stop_start = Instant::now();
                                    while stop_start.elapsed() < grace {
                                        if active_workers
                                            .iter()
                                            .all(|(_, r)| !r.load(Ordering::Relaxed))
                                        {
                                            break;
                                        }
                                        std::thread::sleep(Duration::from_millis(100));
                                    }
                                }

                                // Wait for workers to finish
                                for (h, _) in active_workers {
                                    let _ = h.join();
                                }

                                println!("[Scenario: {}] Complete.", scenario_name);
                            }

                            // Closed-model executors (constant-vus, per-vu-iterations, shared-iterations)
                            _ => {
                                println!(
                                    "[Scenario: {}] Starting {} workers for {}...",
                                    scenario_name, workers, duration
                                );

                                let start_time = Instant::now();
                                let mut active_workers: Vec<(
                                    may::coroutine::JoinHandle<()>,
                                    Arc<AtomicBool>,
                                )> = Vec::new();
                                let mut next_worker_id = 1;

                                // For shared-iterations, create a shared counter across all workers
                                let shared_iteration_counter = Arc::new(AtomicU64::new(0));
                                let total_shared_iterations =
                                    if executor == ExecutorType::SharedIterations {
                                        max_iterations.unwrap_or(workers as u64 * 10)
                                    } else {
                                        0
                                    };
                                let is_shared_iterations =
                                    executor == ExecutorType::SharedIterations;

                                // Spawn all workers at once (constant-vus style)
                                for _ in 0..workers {
                                    let worker_id = next_worker_id;
                                    next_worker_id += 1;
                                    let tx = tx.clone();
                                    let shared_data = shared_data.clone();

                                    let agg = aggregator.clone();
                                    let running = Arc::new(AtomicBool::new(true));
                                    let running_clone = running.clone();
                                    let script_content = script_content.clone();
                                    let script_path = script_path.clone();
                                    let setup_data_clone = setup_data.clone();
                                    let exec_fn = exec_fn.clone();
                                    let scenario_name = scenario_name.clone();
                                    let jitter = jitter.clone();
                                    let drop_rate = drop_rate;
                                    // Use worker-scaled stack for green threads (configurable)
                                    let stack_sz =
                                        scenario_config.stack_size.unwrap_or(may_stack_size);
                                    let response_sink =
                                        scenario_config.response_sink.unwrap_or(false);
                                    let tokio_rt = shared_tokio_rt.clone();
                                    let client = shared_http_client.clone();
                                    let control_state = control_state.clone();
                                    let shared_iter_counter = shared_iteration_counter.clone();
                                    let total_shared_iter = total_shared_iterations;
                                    let is_shared_iter = is_shared_iterations;

                                    // Spawn as may coroutine instead of OS thread
                                    // SAFETY: Stack size is configured to be sufficient for JS execution
                                    let worker_handle = unsafe {
                                        may::coroutine::Builder::new()
                                .stack_size(stack_sz)
                                .spawn(move || {
                                let (runtime, context) = Self::create_runtime().unwrap();
                                runtime.set_memory_limit(256 * 1024); // 256KB per worker max JS heap
                                context.with(|ctx| {
                                    // Use standard HTTP path with hyper/Tokio (better connection pooling)
                                    crate::bridge::register_globals_sync(&ctx, tx.clone(), client, shared_data, worker_id, agg, tokio_rt, jitter, drop_rate, response_sink).unwrap();
                                    // Set scenario name global for automatic tagging
                                    ctx.globals().set("__SCENARIO", scenario_name.clone()).unwrap();

                                    // Module declaration with better error handling
                                    let module = match Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content).catch(&ctx) {
                                        Ok(m) => m,
                                        Err(e) => {
                                            eprintln!("[Scenario: {}] Script error:\n{}", scenario_name, format_js_error(&e));
                                            return;
                                        }
                                    };
                                    let (module, _) = match module.eval().catch(&ctx) {
                                        Ok(m) => m,
                                        Err(e) => {
                                            eprintln!("[Scenario: {}] Script error:\n{}", scenario_name, format_js_error(&e));
                                            return;
                                        }
                                    };

                                    // Get the exec function (default or custom)
                                    let func: Function = match module.get(&exec_fn) {
                                        Ok(f) => f,
                                        Err(_) => {
                                            eprintln!("[Scenario: {}] Function '{}' not found, trying 'default'", scenario_name, exec_fn);
                                            match module.get("default") {
                                                Ok(f) => f,
                                                Err(_) => {
                                                    eprintln!("[Scenario: {}] No 'default' function found either. Worker exiting.", scenario_name);
                                                    return;
                                                }
                                            }
                                        }
                                    };

                                    let data_val = if let Some(json) = setup_data_clone.as_ref() {
                                        json_parse(ctx.clone(), json).unwrap_or(Value::new_undefined(ctx.clone()))
                                    } else {
                                        Value::new_undefined(ctx.clone())
                                    };

                                    let mut iteration_count: u64 = 0;
                                    while running_clone.load(Ordering::Relaxed) {
                                        // Pause logic - use may sleep to yield coroutine
                                        while control_state.is_paused() && running_clone.load(Ordering::Relaxed) {
                                            may::coroutine::sleep(Duration::from_millis(100));
                                        }
                                        if control_state.is_stopped() || !running_clone.load(Ordering::Relaxed) {
                                            break;
                                        }

                                        // Reset sleep tracker before each iteration
                                        crate::bridge::reset_sleep_tracker();
                                        let _ = ctx.globals().set("__ITERATION", iteration_count);
                                        let iter_start = Instant::now();
                                        let call_result = func.call::<_, ()>((data_val.clone(),)).catch(&ctx);
                                        let elapsed = iter_start.elapsed(); // Total elapsed time including sleep
                                        let sleep_time = crate::bridge::get_sleep_accumulated();
                                        let active_time = elapsed.saturating_sub(sleep_time); // Actual work time excluding sleep

                                        // Primary metric: iteration (active time only - excludes sleep)
                                        let timings = crate::stats::RequestTimings { duration: active_time, ..Default::default() };
                                        // Secondary metric: iteration_total (includes sleep for pacing analysis)
                                        let timings_total = crate::stats::RequestTimings { duration: elapsed, ..Default::default() };

                                        // Get tags
                                        let tags: HashMap<String, String> = control_state.get_tags();

                                        match call_result {
                                            Ok(_) => {
                                                // Send active time metric (default - what users want to measure)
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration", scenario_name),
                                                    timings,
                                                    status: 200,
                                                    error: None,
                                                    tags: tags.clone(),
                                                });
                                                // Send total time metric (includes sleep for throughput/pacing analysis)
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration_total", scenario_name),
                                                    timings: timings_total,
                                                    status: 200,
                                                    error: None,
                                                    tags,
                                                });
                                            },
                                            Err(e) => {
                                                let error_msg = format_js_error(&e);
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration", scenario_name),
                                                    timings,
                                                    status: 0,
                                                    error: Some(error_msg.clone()),
                                                    tags: tags.clone(),
                                                });
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration_total", scenario_name),
                                                    timings: timings_total,
                                                    status: 0,
                                                    error: Some(error_msg),
                                                    tags,
                                                });
                                            }
                                        }

                                        iteration_count += 1;

                                        // Check iteration limits based on executor type
                                        if is_shared_iter {
                                            // SharedIterations: claim from shared counter
                                            let claimed = shared_iter_counter.fetch_add(1, Ordering::SeqCst);
                                            if claimed >= total_shared_iter {
                                                break; // All iterations consumed
                                            }
                                        } else if let Some(max_iter) = max_iterations {
                                            // PerVuIterations: each worker has its own limit
                                            if iteration_count >= max_iter {
                                                break;
                                            }
                                        }
                                        if let Some(min_dur) = min_iter_duration {
                                            if elapsed < min_dur {
                                                may::coroutine::sleep(min_dur - elapsed);
                                            }
                                        }
                                        // No artificial delay - let async I/O provide natural pacing
                                        // This allows maximum throughput when min_iteration_duration is not set
                                    }
                                                });
                                                // Cleanup: run GC and forget runtime to prevent assertion failures
                                                runtime.run_gc();
                                                std::mem::drop(context);
                                                std::mem::forget(runtime);
                                            })
                                    };
                                    active_workers.push((
                                        worker_handle.expect("Failed to spawn coroutine"),
                                        running,
                                    ));
                                }

                                // Wait for scenario duration or all shared iterations to complete
                                while start_time.elapsed() < scenario_duration {
                                    // Check global stop
                                    if control_state.is_stopped() {
                                        break;
                                    }
                                    // For shared-iterations, exit early when all iterations consumed
                                    if is_shared_iterations
                                        && shared_iteration_counter.load(Ordering::SeqCst)
                                            >= total_shared_iterations
                                    {
                                        break;
                                    }
                                    std::thread::sleep(Duration::from_millis(100));
                                }

                                // Stop all workers
                                for (_, r) in active_workers.iter() {
                                    r.store(false, Ordering::Relaxed);
                                }

                                // Graceful stop: wait for in-flight requests to complete
                                let grace = graceful_stop
                                    .as_deref()
                                    .and_then(parse_duration_str)
                                    .unwrap_or(Duration::from_secs(30));
                                if grace > Duration::ZERO {
                                    let stop_start = Instant::now();
                                    while stop_start.elapsed() < grace {
                                        if active_workers
                                            .iter()
                                            .all(|(_, r)| !r.load(Ordering::Relaxed))
                                        {
                                            break;
                                        }
                                        std::thread::sleep(Duration::from_millis(100));
                                    }
                                }

                                // Wait for workers to finish
                                for (h, _) in active_workers {
                                    let _ = h.join();
                                }

                                println!("[Scenario: {}] Complete.", scenario_name);
                            } // End of closed-model executor match arm
                        } // End of executor match
                    });
                    active_scenario_handles.push(h);
                }

                // Initialization for legacy (empty, unused)
                schedule_legacy = vec![];
                duration_legacy = Duration::ZERO;
            } else {
                // Legacy single-scenario mode setup
                schedule_legacy = if let Some(ref s) = config.schedule {
                    let mut parsed = Vec::new();
                    for step in s {
                        let dur = parse_duration(&step.duration).map_err(|_| {
                            anyhow::anyhow!("Invalid duration in schedule: '{}'", step.duration)
                        })?;
                        parsed.push((dur, step.target));
                    }
                    parsed
                } else {
                    let duration_str = config.duration.as_deref().unwrap_or("10s");
                    let workers = config.workers.unwrap_or(1);
                    let duration = parse_duration(duration_str)
                        .map_err(|_| anyhow::anyhow!("Invalid duration: '{}'", duration_str))?;
                    vec![(Duration::from_secs(0), workers), (duration, workers)]
                };

                duration_legacy = schedule_legacy.iter().map(|(d, _)| *d).sum();
            }

            // UNIFIED CONTROL LOOP

            // If multi-scenario, we loop until all handles are done or stopped
            // If legacy, we loop based on duration_legacy

            // Setup periodic metrics reporting if URL is specified
            let metrics_client = if metrics_url.is_some() {
                Some(HttpClient::new())
            } else {
                None
            };
            let mut last_metrics_send = Instant::now();
            let metrics_interval = Duration::from_secs(5);

            'main_loop: loop {
                if is_multi_scenario {
                    if active_scenario_handles.iter().all(|h| h.is_finished()) {
                        break 'main_loop;
                    }
                } else if start_time
                    .elapsed()
                    .saturating_sub(control_state.total_paused())
                    >= duration_legacy
                    && !control_state.is_stopped()
                {
                    // Only break if natural finish (exclude paused time)
                    break 'main_loop;
                }

                if control_state.is_stopped() {
                    break 'main_loop;
                }

                // Process control commands
                if let Some(ref rx) = control_rx {
                    while let Ok(cmd) = rx.try_recv() {
                        let tui_on =
                            crate::bridge::TUI_ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
                        match cmd {
                            control::ControlCommand::Ramp(0) => {
                                // Ramp 0 = return to schedule
                                if !tui_on {
                                    println!("[Control] Returning to schedule.");
                                }
                                dynamic_target_legacy = None;
                            }
                            control::ControlCommand::Ramp(n) => {
                                if is_multi_scenario {
                                    if !tui_on {
                                        println!("[Control] WARNING: Ramping not supported in multi-scenario mode.");
                                    }
                                } else {
                                    if !tui_on {
                                        println!("[Control] Ramping to {} workers", n);
                                    }
                                    control_state.set_target_workers(n);
                                    dynamic_target_legacy = Some(n);
                                }
                            }
                            control::ControlCommand::Pause => {
                                if !tui_on {
                                    println!("[Control] Pausing workers...");
                                }
                                control_state.pause();
                            }
                            control::ControlCommand::Resume => {
                                if !tui_on {
                                    println!("[Control] Resuming workers...");
                                }
                                control_state.resume();
                            }
                            control::ControlCommand::Tag(k, v) => {
                                if !tui_on {
                                    println!("[Control] Adding tag: {}={}", k, v);
                                }
                                control_state.add_tag(k, v);
                            }
                            control::ControlCommand::Status => {
                                if !tui_on {
                                    let paused = if control_state.is_paused() {
                                        "PAUSED"
                                    } else {
                                        "RUNNING"
                                    };
                                    let vus = if is_multi_scenario {
                                        "many".to_string()
                                    } else {
                                        active_legacy_workers.len().to_string()
                                    };
                                    println!("[Status] Workers: {}, State: {}", vus, paused);
                                }
                            }
                            control::ControlCommand::Stop => {
                                if !tui_on {
                                    println!("[Control] Stopping test...");
                                }
                                control_state.stop();
                                break 'main_loop;
                            }
                        }
                    }
                }

                if !is_multi_scenario && !control_state.is_stopped() {
                    // Legacy scaling logic (exclude paused time)
                    let elapsed = start_time
                        .elapsed()
                        .saturating_sub(control_state.total_paused());
                    let scheduled_target = Self::calculate_target(&schedule_legacy, elapsed);
                    let target_workers = dynamic_target_legacy.unwrap_or(scheduled_target);

                    if target_workers > active_legacy_workers.len() {
                        // Memory-safe mode: Skip spawning if memory is throttled
                        if memory_throttle.load(Ordering::Relaxed) {
                            // Check if memory has recovered (below warning threshold)
                            let info = memory::MemoryInfo::current();
                            if info.usage_percent() < memory::WARN_THRESHOLD_PERCENT {
                                memory_throttle.store(false, Ordering::Relaxed);
                                memory_critical.store(false, Ordering::Relaxed);
                                eprintln!(
                                    "[Memory] Recovered: {:.1}% used. Resuming worker spawning.",
                                    info.usage_percent() * 100.0
                                );
                            } else {
                                // Still throttled, skip spawning this tick
                                std::thread::sleep(Duration::from_millis(100));
                                continue;
                            }
                        }

                        for _ in 0..(target_workers - active_legacy_workers.len()) {
                            let worker_id = next_worker_id;
                            next_worker_id += 1;
                            let tx = tx.clone();
                            let shared_data = shared_data.clone();

                            let agg = aggregator.clone();
                            let running = Arc::new(AtomicBool::new(true));
                            let running_clone = running.clone();
                            let script_content = script_content.clone();
                            let script_path = script_path.clone();
                            let setup_data_clone = setup_data.clone();
                            let max_iterations = config.iterations;
                            let jitter = config.jitter.clone();
                            let drop_rate = config.drop;
                            let response_sink = config.response_sink.unwrap_or(false);
                            let control_state = control_state.clone();
                            // Legacy mode: use tiered stack sizes same as multi-scenario
                            let stack_sz = config.stack_size.unwrap_or(may_stack_size);
                            let tokio_rt = shared_tokio_rt.clone();
                            let client = shared_http_client.clone();

                            // Spawn as may coroutine instead of OS thread for higher concurrency
                            // SAFETY: Stack size is configured to be sufficient for JS execution
                            let h = unsafe {
                                may::coroutine::Builder::new()
                                .stack_size(stack_sz)
                                .spawn(move || {
                                let (runtime, context) = Self::create_runtime().unwrap();
                                runtime.set_memory_limit(256 * 1024); // 256KB per worker max JS heap
                                context.with(|ctx| {
                                    // Pass std::sync::mpsc::Sender directly - no bridge thread needed!
                                    // Clone tx since we need it for both bridge and iteration metrics
                                    crate::bridge::register_globals_sync(&ctx, tx.clone(), client, shared_data, worker_id, agg, tokio_rt, jitter, drop_rate, response_sink).unwrap();

                                    // Module declaration with better error handling
                                    let module = match Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content).catch(&ctx) {
                                        Ok(m) => m,
                                        Err(e) => {
                                            eprintln!("Script error:\n{}", format_js_error(&e));
                                            return;
                                        }
                                    };
                                    let (module, _) = match module.eval().catch(&ctx) {
                                        Ok(m) => m,
                                        Err(e) => {
                                            eprintln!("Script error:\n{}", format_js_error(&e));
                                            return;
                                        }
                                    };
                                    let func: Function = match module.get("default") {
                                        Ok(f) => f,
                                        Err(_) => {
                                            eprintln!("Script error: 'default' function not found. Did you forget to export default function?");
                                            return;
                                        }
                                    };

                                    // Parse setup data once
                                    let data_val = if let Some(json) = setup_data_clone.as_ref() {
                                        json_parse(ctx.clone(), json).unwrap_or(Value::new_undefined(ctx.clone()))
                                    } else {
                                        Value::new_undefined(ctx.clone())
                                    };

                                    let mut iteration_count: u64 = 0;
                                    while running_clone.load(Ordering::Relaxed) {
                                        // Pause logic - use may sleep to yield coroutine
                                        while control_state.is_paused() && running_clone.load(Ordering::Relaxed) {
                                            may::coroutine::sleep(Duration::from_millis(100));
                                        }
                                        if control_state.is_stopped() || !running_clone.load(Ordering::Relaxed) {
                                            break;
                                        }

                                        // Reset sleep tracker before each iteration
                                        crate::bridge::reset_sleep_tracker();
                                        let _ = ctx.globals().set("__ITERATION", iteration_count);
                                        let iter_start = Instant::now();
                                        let call_result = func.call::<_, ()>((data_val.clone(),)).catch(&ctx);
                                        let elapsed = iter_start.elapsed(); // Total elapsed time including sleep
                                        let sleep_time = crate::bridge::get_sleep_accumulated();
                                        let active_time = elapsed.saturating_sub(sleep_time); // Actual work time excluding sleep

                                        // Primary metric: iteration (active time only - excludes sleep)
                                        let timings = crate::stats::RequestTimings { duration: active_time, ..Default::default() };
                                        // Secondary metric: iteration_total (includes sleep for pacing analysis)
                                        let timings_total = crate::stats::RequestTimings { duration: elapsed, ..Default::default() };

                                        // Get tags
                                        let tags: HashMap<String, String> = control_state.get_tags();

                                        match call_result {
                                            Ok(_) => {
                                                // Send active time metric (default - what users want to measure)
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration".to_string(),
                                                    timings,
                                                    status: 200,
                                                    error: None,
                                                    tags: tags.clone(),
                                                });
                                                // Send total time metric (includes sleep for throughput/pacing analysis)
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration_total".to_string(),
                                                    timings: timings_total,
                                                    status: 200,
                                                    error: None,
                                                    tags,
                                                });
                                            },
                                            Err(e) => {
                                                let error_msg = format_js_error(&e);
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration".to_string(),
                                                    timings,
                                                    status: 0,
                                                    error: Some(error_msg.clone()),
                                                    tags: tags.clone(),
                                                });
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration_total".to_string(),
                                                    timings: timings_total,
                                                    status: 0,
                                                    error: Some(error_msg),
                                                    tags,
                                                });
                                            }
                                        }

                                        iteration_count += 1;
                                        // Check iteration limit (per-vu-iterations executor)
                                        if let Some(max_iter) = max_iterations {
                                            if iteration_count >= max_iter {
                                                break;
                                            }
                                        }
                                        if let Some(min_dur) = min_iter_duration {
                                            if elapsed < min_dur {
                                                may::coroutine::sleep(min_dur - elapsed);
                                            }
                                        }
                                        // No artificial delay - let async I/O provide natural pacing
                                    }
                                });
                                // Cleanup: run GC and forget runtime to prevent assertion failures
                                runtime.run_gc();
                                std::mem::drop(context);
                                std::mem::forget(runtime);
                            })
                            };
                            active_legacy_workers
                                .push((h.expect("Failed to spawn coroutine"), running));
                        }
                    }

                    // Scale down if target is less than current (only from ramp command)
                    if let Some(target) = dynamic_target_legacy {
                        while target < active_legacy_workers.len()
                            && !active_legacy_workers.is_empty()
                        {
                            if let Some((_, running)) = active_legacy_workers.pop() {
                                running.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                } // End legacy scaling for this loop tick

                // Periodic metrics reporting to external URL
                if let (Some(ref url), Some(ref client)) = (&metrics_url, &metrics_client) {
                    if last_metrics_send.elapsed() >= metrics_interval {
                        // Merge sharded aggregator and convert to report for accurate stats
                        let merged = sharded_aggregator.merge();
                        let report = merged.to_report();
                        let elapsed_secs = start_time
                            .elapsed()
                            .saturating_sub(control_state.total_paused())
                            .as_secs_f64()
                            .max(0.001);
                        let rps = report.total_requests as f64 / elapsed_secs;

                        // Calculate error count
                        let error_count: usize = report.errors.values().sum();

                        // Calculate active workers
                        let active_workers = if is_multi_scenario {
                            total_workers as i32 // Approximate for multi-scenario
                        } else {
                            active_legacy_workers.len() as i32
                        };

                        // Build metrics payload matching control plane format
                        // Note: Endpoint metrics are sent separately at completion to avoid performance overhead
                        let payload = serde_json::json!({
                            "requests_total": report.total_requests,
                            "requests_per_sec": rps,
                            "latency_avg_ms": report.avg_latency_ms,
                            "latency_p95_ms": report.p95_latency_ms,
                            "errors": error_count,
                            "active_workers": active_workers,
                            "data_sent": report.total_data_sent,
                            "data_received": report.total_data_received
                        });

                        // Send metrics to URL
                        let body = payload.to_string();
                        let mut req_builder = http::Request::builder()
                            .method(Method::POST)
                            .uri(url)
                            .header("Content-Type", "application/json");

                        // Add auth header if provided (format: "HeaderName: value")
                        if let Some(ref auth) = metrics_auth {
                            if let Some((header_name, header_value)) = auth.split_once(':') {
                                req_builder =
                                    req_builder.header(header_name.trim(), header_value.trim());
                            }
                        }

                        if let Ok(req) = req_builder.body(body) {
                            let _ = shared_tokio_rt.block_on(client.request(req, false));
                        }
                        last_metrics_send = Instant::now();
                    }
                }

                std::thread::sleep(Duration::from_millis(100));
            } // End main_loop

            // Signal TUI that the test is done
            control_state.stop();

            // Clean up
            if is_multi_scenario {
                // Wait for all scenarios to complete
                for h in active_scenario_handles {
                    let _ = h.join();
                }
            } else {
                for (_, r) in active_legacy_workers.iter() {
                    r.store(false, Ordering::Relaxed);
                }
                // Graceful Stop for legacy
                let graceful_stop_duration = config
                    .stop
                    .as_deref()
                    .and_then(parse_duration_str)
                    .unwrap_or(Duration::from_secs(30));

                if graceful_stop_duration > Duration::ZERO {
                    println!(
                        "Graceful stop: waiting up to {}s for workers to finish...",
                        graceful_stop_duration.as_secs_f64()
                    );
                    let stop_start = Instant::now();
                    while stop_start.elapsed() < graceful_stop_duration {
                        // Check if all workers have stopped (running flag is false means iteration loop exited)
                        let all_stopped = active_legacy_workers
                            .iter()
                            .all(|(_, r)| !r.load(Ordering::Relaxed));
                        if all_stopped {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }

            // Merge all shards into final aggregator for reporting
            let merged_agg = sharded_aggregator.merge();
            let report = merged_agg.to_report();

            if json_output {
                println!("{}", merged_agg.to_json());
            } else {
                merged_agg.report();
            }

            if let Some(ref path) = export_json {
                if let Err(e) = std::fs::write(path, merged_agg.to_json()) {
                    eprintln!("Failed to export JSON to {}: {}", path.display(), e);
                }
            }

            if let Some(ref path) = export_html {
                let html = crate::stats::html::generate_html(&report);
                if let Err(e) = std::fs::write(path, html) {
                    eprintln!("Failed to export HTML to {}: {}", path.display(), e);
                }
            }

            // Send final summary with endpoint metrics to control plane
            if let (Some(ref url), Some(ref client)) = (&metrics_url, &metrics_client) {
                // Build endpoint metrics for per-URL breakdown (sent once at completion)
                let endpoints: Vec<serde_json::Value> = report
                    .grouped_requests
                    .iter()
                    .map(|(name, req_report)| {
                        serde_json::json!({
                            "name": name,
                            "requests": req_report.total_requests,
                            "avg_latency_ms": req_report.avg_latency_ms,
                            "p95_latency_ms": req_report.p95_latency_ms,
                            "min_latency_ms": req_report.min_latency_ms,
                            "max_latency_ms": req_report.max_latency_ms,
                            "errors": req_report.error_count
                        })
                    })
                    .collect();

                if !endpoints.is_empty() {
                    // Include global status codes breakdown
                    let status_codes: std::collections::HashMap<String, usize> = report
                        .status_codes
                        .iter()
                        .map(|(code, count)| (code.to_string(), *count))
                        .collect();

                    let summary_payload = serde_json::json!({
                        "endpoints": endpoints,
                        "status_codes": status_codes
                    });
                    let summary_url = url.replace("/metrics", "/summary");

                    let mut req_builder = http::Request::builder()
                        .method(Method::POST)
                        .uri(&summary_url)
                        .header("Content-Type", "application/json");

                    if let Some(ref auth) = metrics_auth {
                        if let Some((header_name, header_value)) = auth.split_once(':') {
                            req_builder =
                                req_builder.header(header_name.trim(), header_value.trim());
                        }
                    }

                    if let Ok(req) = req_builder.body(summary_payload.to_string()) {
                        let _ = shared_tokio_rt.block_on(client.request(req, false));
                    }
                }
            }

            // Save to history DB
            if let Ok(db) = crate::stats::db::HistoryDb::open_default() {
                let _ = db.save_run(&script_path.to_string_lossy(), &report);
            }

            // Teardown
            let _ = self.run_teardown(
                &script_path,
                &script_content,
                setup_data.as_deref().map(|s| s.to_string()),
                aggregator.clone(),
            );

            // Run handleSummary hook if defined
            let test_duration = start_time.elapsed();
            let _ = self.run_handle_summary(
                &script_path,
                &script_content,
                &report,
                test_duration,
                aggregator.clone(),
            );

            // Validate thresholds if criteria are defined
            let mut threshold_failures = if let Some(ref criteria) = config.criteria {
                merged_agg.validate_thresholds(criteria)
            } else {
                Vec::new()
            };

            // Per-scenario threshold evaluation
            if let Some(ref scenarios) = config.scenarios {
                for (scenario_name, scenario_config) in scenarios {
                    if let Some(ref thresholds) = scenario_config.thresholds {
                        let scenario_failures =
                            merged_agg.validate_scenario_thresholds(scenario_name, thresholds);
                        threshold_failures.extend(scenario_failures);
                    }
                }
            }

            // If abort_on_fail is enabled and thresholds failed, print failures and return error
            if config.abort_on_fail.unwrap_or(false) && !threshold_failures.is_empty() {
                eprintln!("\nThreshold failures:");
                for failure in &threshold_failures {
                    eprintln!("  {}", failure);
                }
                return Err(anyhow::anyhow!(
                    "Thresholds breached: {} failure(s)",
                    threshold_failures.len()
                ));
            } else if !threshold_failures.is_empty() {
                // Print threshold failures as warnings even if not aborting
                eprintln!("\nThreshold failures (warnings):");
                for failure in &threshold_failures {
                    eprintln!("  {}", failure);
                }
            }

            Ok(report)
        });

        handle
            .join()
            .map_err(|_| anyhow::anyhow!("Test thread panicked"))?
    }

    fn calculate_target(schedule: &[(Duration, usize)], elapsed: Duration) -> usize {
        let mut active_time = Duration::from_secs(0);
        let mut prev_target = 0;
        for (duration, target) in schedule {
            if elapsed < active_time + *duration {
                let progress =
                    (elapsed.as_secs_f64() - active_time.as_secs_f64()) / duration.as_secs_f64();
                let diff = *target as f64 - prev_target as f64;
                return (prev_target as f64 + diff * progress) as usize;
            }
            active_time += *duration;
            prev_target = *target;
        }
        0
    }

    /// Calculate the current rate for ramping-arrival-rate executor.
    /// Similar to calculate_target but works with u64 rate values.
    fn calculate_rate(schedule: &[(Duration, u64)], elapsed: Duration) -> u64 {
        let mut active_time = Duration::from_secs(0);
        let mut prev_rate: u64 = 0;
        for (duration, rate) in schedule {
            if elapsed < active_time + *duration {
                let progress =
                    (elapsed.as_secs_f64() - active_time.as_secs_f64()) / duration.as_secs_f64();
                let diff = *rate as f64 - prev_rate as f64;
                return (prev_rate as f64 + diff * progress) as u64;
            }
            active_time += *duration;
            prev_rate = *rate;
        }
        prev_rate
    }

    pub fn warmup(&self, url: &str) {
        let url = url.to_string();
        // Spawn a few threads to establish multiple connections in the pool
        let threads: Vec<_> = (0..10)
            .map(|_| {
                let u = url.clone();
                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let client = {
                        let _guard = rt.enter();
                        HttpClient::new()
                    };
                    let req = http::Request::builder()
                        .method(Method::HEAD)
                        .uri(&u)
                        .body("".to_string());

                    if let Ok(r) = req {
                        let _ = rt.block_on(async move {
                            tokio::time::timeout(Duration::from_secs(10), client.request(r, false))
                                .await
                        });
                    }
                })
            })
            .collect();

        for t in threads {
            let _ = t.join();
        }
    }

    fn run_setup(
        &self,
        script_path: &Path,
        script_content: &str,
        aggregator: SharedAggregator,
    ) -> Result<Option<String>> {
        let (runtime, context) = Self::create_runtime()?;
        let (_tx, _rx) = crossbeam_channel::unbounded::<Metric>();
        let (tx_print, rx_print) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Metric::Log { message }) = rx_print.recv() {
                println!("{}", message);
            }
        });

        let shared_data = self.shared_data.clone();
        let tokio_rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        );
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        let result = context.with(|ctx| {
            crate::bridge::register_globals_sync(
                &ctx,
                tx_print,
                client,
                shared_data,
                0,
                aggregator,
                tokio_rt,
                None,
                None,
                false,
            )?;
            let module = Module::declare(
                ctx.clone(),
                script_path.to_string_lossy().to_string(),
                script_content,
            )?;
            let (module, _) = module.eval()?;

            if let Ok(setup_fn) = module.get::<_, Function>("setup") {
                let result = setup_fn.call::<_, Value>(())?;
                if !result.is_undefined() {
                    return Ok(Some(json_stringify(ctx.clone(), result)?));
                }
            }
            Ok(None)
        });

        // Cleanup: run GC and forget runtime to prevent assertion failures
        runtime.run_gc();
        drop(context);
        std::mem::forget(runtime);
        result
    }

    pub fn run_script(&self, script_content: &str) -> Result<()> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx, _rx) = crossbeam_channel::unbounded::<Metric>();
        let shared_data = self.shared_data.clone();
        let aggregator = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));
        let tokio_rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        );
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        context.with(|ctx| {
            crate::bridge::register_globals_sync(
                &ctx,
                tx,
                client,
                shared_data,
                0,
                aggregator,
                tokio_rt,
                None,
                None,
                false,
            )?;
            let _ = ctx.eval::<Value, _>(script_content)?;
            Ok::<(), anyhow::Error>(())
        })?;

        // Cleanup: run GC and forget runtime to prevent assertion failures
        runtime.run_gc();
        drop(context);
        std::mem::forget(runtime);
        Ok(())
    }

    fn run_teardown(
        &self,
        script_path: &Path,
        script_content: &str,
        data: Option<String>,
        aggregator: SharedAggregator,
    ) -> Result<()> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx_print, rx_print) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Metric::Log { message }) = rx_print.recv() {
                println!("{}", message);
            }
        });

        let shared_data = self.shared_data.clone();
        let tokio_rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        );
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        let _ = context.with(|ctx| {
            crate::bridge::register_globals_sync(
                &ctx,
                tx_print,
                client,
                shared_data,
                0,
                aggregator,
                tokio_rt,
                None,
                None,
                false,
            )?;
            let module = Module::declare(
                ctx.clone(),
                script_path.to_string_lossy().to_string(),
                script_content,
            )?;
            let (module, _) = module.eval()?;

            if let Ok(teardown_fn) = module.get::<_, Function>("teardown") {
                let data_val = if let Some(json) = data {
                    json_parse(ctx.clone(), &json).unwrap_or(Value::new_undefined(ctx.clone()))
                } else {
                    Value::new_undefined(ctx.clone())
                };
                let _ = teardown_fn.call::<_, ()>((data_val,));
            }
            Ok::<(), anyhow::Error>(())
        });

        // Cleanup: run GC and forget runtime to prevent assertion failures
        runtime.run_gc();
        drop(context);
        std::mem::forget(runtime);
        Ok(())
    }

    /// Run the handleSummary function if exported by the script.
    /// The function receives a data object with metrics and can return
    /// a map of file paths to content to write.
    fn run_handle_summary(
        &self,
        script_path: &Path,
        script_content: &str,
        report: &crate::stats::ReportStats,
        test_duration: Duration,
        aggregator: SharedAggregator,
    ) -> Result<()> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx_print, rx_print) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Metric::Log { message }) = rx_print.recv() {
                println!("{}", message);
            }
        });

        let shared_data = self.shared_data.clone();
        let tokio_rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?,
        );
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        // Serialize report to JSON for passing to JS
        let report_json = serde_json::to_string(report)?;
        let test_duration_str = format_duration(test_duration);

        let _ = context.with(|ctx| {
            crate::bridge::register_globals_sync(
                &ctx,
                tx_print,
                client,
                shared_data,
                0,
                aggregator,
                tokio_rt,
                None,
                None,
                false,
            )?;
            let module = Module::declare(
                ctx.clone(),
                script_path.to_string_lossy().to_string(),
                script_content,
            )?;
            let (module, _) = module.eval()?;

            // Check if handleSummary function is exported
            if let Ok(handle_summary_fn) = module.get::<_, Function>("handleSummary") {
                // Build the summary data object
                let data = json_parse(ctx.clone(), &report_json)
                    .unwrap_or_else(|_| Value::new_undefined(ctx.clone()));

                // Add state information to the data object
                if let Some(data_obj) = data.as_object() {
                    // Create state object
                    let state = Object::new(ctx.clone())?;
                    state.set("isStdOutTTY", atty::is(atty::Stream::Stdout))?;
                    state.set("testRunDuration", test_duration_str)?;
                    data_obj.set("state", state)?;
                }

                // Call handleSummary with the data
                let result: Value = handle_summary_fn.call((data,)).catch(&ctx).map_err(|e| {
                    anyhow::anyhow!("handleSummary failed: {}", format_js_error(&e))
                })?;

                // Process the return value
                // Expected format: { 'stdout': 'text', 'filename.json': 'content', ... }
                if let Some(obj) = result.as_object() {
                    for key in obj.keys::<String>() {
                        let key = key?;
                        // Get the value - could be string or object
                        let val: Value = obj.get(&key)?;
                        let content = if val.is_string() {
                            val.as_string()
                                .and_then(|s| s.to_string().ok())
                                .unwrap_or_default()
                        } else {
                            // Try to stringify non-string values
                            json_stringify(ctx.clone(), val).unwrap_or_default()
                        };

                        if key == "stdout" {
                            // Print to console
                            print!("{}", content);
                        } else if key == "stderr" {
                            // Print to stderr
                            eprint!("{}", content);
                        } else {
                            // Write to file
                            if let Err(e) = std::fs::write(&key, &content) {
                                eprintln!("handleSummary: failed to write to {}: {}", key, e);
                            }
                        }
                    }
                }
            }
            Ok::<(), anyhow::Error>(())
        });

        // Cleanup: run GC and forget runtime to prevent assertion failures
        runtime.run_gc();
        drop(context);
        std::mem::forget(runtime);
        Ok(())
    }
}

/// Format duration in human-readable format (e.g., "1m30s", "45s", "250ms")
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let millis = d.subsec_millis();

    if total_secs == 0 {
        format!("{}ms", millis)
    } else if total_secs < 60 {
        if millis > 0 {
            format!("{}s{}ms", total_secs, millis)
        } else {
            format!("{}s", total_secs)
        }
    } else {
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        if secs > 0 {
            format!("{}m{}s", mins, secs)
        } else {
            format!("{}m", mins)
        }
    }
}

fn parse_duration(d: &str) -> Result<Duration> {
    if d.ends_with("ms") {
        let ms = d.trim_end_matches("ms").parse::<u64>()?;
        Ok(Duration::from_millis(ms))
    } else if d.ends_with('s') {
        let secs = d.trim_end_matches('s').parse::<u64>()?;
        Ok(Duration::from_secs(secs))
    } else if d.ends_with('m') {
        let mins = d.trim_end_matches('m').parse::<u64>()?;
        Ok(Duration::from_secs(mins * 60))
    } else {
        anyhow::bail!("Invalid duration format: {}", d)
    }
}

fn json_stringify<'js>(ctx: Ctx<'js>, value: Value<'js>) -> Result<String, rquickjs::Error> {
    let json_obj: Object = ctx.globals().get("JSON")?;
    let stringify: Function = json_obj.get("stringify")?;
    let json_str: String = stringify.call((value,))?;
    Ok(json_str)
}

#[allow(dead_code)]
fn json_parse<'js>(ctx: Ctx<'js>, json: &str) -> Result<Value<'js>, rquickjs::Error> {
    let json_obj: Object = ctx.globals().get("JSON")?;
    let parse: Function = json_obj.get("parse")?;
    parse.call((json,))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
    }

    #[test]
    fn test_calculate_rate_constant() {
        // For a constant rate schedule, should always return the same rate
        let schedule = vec![(Duration::from_secs(60), 100u64)];

        assert_eq!(Engine::calculate_rate(&schedule, Duration::from_secs(0)), 0);
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(30)),
            50
        );
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(60)),
            100
        );
    }

    #[test]
    fn test_calculate_rate_ramping() {
        // Ramping from 0 -> 100 over 10s, then hold at 100 for 10s, then ramp down to 0 over 10s
        let schedule = vec![
            (Duration::from_secs(10), 100u64),
            (Duration::from_secs(10), 100u64),
            (Duration::from_secs(10), 0u64),
        ];

        // First stage: ramping up
        assert_eq!(Engine::calculate_rate(&schedule, Duration::from_secs(0)), 0);
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(5)),
            50
        );
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(10)),
            100
        );

        // Second stage: holding at 100
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(15)),
            100
        );
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(20)),
            100
        );

        // Third stage: ramping down
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(25)),
            50
        );
        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(30)),
            0
        );
    }

    #[test]
    fn test_calculate_rate_after_schedule() {
        // After schedule ends, should return the last rate
        let schedule = vec![(Duration::from_secs(10), 100u64)];

        assert_eq!(
            Engine::calculate_rate(&schedule, Duration::from_secs(100)),
            100
        );
    }

    #[test]
    fn test_validate_scenario_name_valid() {
        assert!(validate_scenario_name("default").is_ok());
        assert!(validate_scenario_name("my_scenario").is_ok());
        assert!(validate_scenario_name("scenario-1").is_ok());
        assert!(validate_scenario_name("test.scenario").is_ok());
        assert!(validate_scenario_name("MyScenario123").is_ok());
    }

    #[test]
    fn test_validate_scenario_name_empty() {
        assert!(validate_scenario_name("").is_err());
    }

    #[test]
    fn test_validate_scenario_name_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_scenario_name(&long_name).is_err());

        // Exactly 64 should be fine
        let max_name = "a".repeat(64);
        assert!(validate_scenario_name(&max_name).is_ok());
    }

    #[test]
    fn test_validate_scenario_name_invalid_chars() {
        assert!(validate_scenario_name("my scenario").is_err()); // space
        assert!(validate_scenario_name("test/scenario").is_err()); // slash
        assert!(validate_scenario_name("test:scenario").is_err()); // colon
        assert!(validate_scenario_name("test@scenario").is_err()); // at
        assert!(validate_scenario_name("test#scenario").is_err()); // hash
    }

    #[test]
    fn test_pre_validation_syntax_error() {
        // Test that script syntax errors are caught during validation
        let engine = Engine::new().unwrap();

        // Script with syntax error - missing closing brace
        let script_with_error = r#"
export const options = { workers: 1, duration: '1s' };
export default function() {
    // Missing closing brace
"#;

        // Use extract_config which creates a runtime and compiles the script
        let result = engine.extract_config(
            std::path::PathBuf::from("test.js"),
            script_with_error.to_string(),
        );

        // Should fail with an error containing info about the syntax issue
        assert!(
            result.is_err(),
            "Expected error for script with syntax error"
        );
    }

    #[test]
    fn test_pre_validation_valid_script() {
        let engine = Engine::new().unwrap();

        let valid_script = r#"
export const options = { workers: 1, duration: '1s' };
export default function() {
    console.log('test');
}
"#;

        let result = engine.extract_config(
            std::path::PathBuf::from("test.js"),
            valid_script.to_string(),
        );

        // Should succeed
        assert!(
            result.is_ok(),
            "Expected success for valid script, got: {:?}",
            result
        );
    }
}
