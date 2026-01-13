use anyhow::Result;
use rquickjs::{Context, Runtime, Module, Function, Value, Object, Ctx, loader::{FileResolver, ScriptLoader}, CatchResultExt};
use tokio::time::{Duration, Instant};
use crate::cli::config::Config;
use crate::stats::{Metric, StatsAggregator, SharedAggregator, ShardedAggregator};
use crate::engine::distributed::MetricBatch;
use crate::engine::io_bridge::IoBridge;
use crossbeam_channel::{self, Sender, Receiver};
use std::collections::HashMap;
use std::path::{PathBuf, Path};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use crate::engine::http_client::HttpClient;
use std::thread::JoinHandle;
use http::Method;
// Will use may::coroutine when worker spawning is migrated
#[allow(unused_imports)]
use may::coroutine;



fn parse_duration_str(s: &str) -> Option<Duration> {
    if s.ends_with("ms") {
        s.trim_end_matches("ms").parse::<u64>().ok().map(Duration::from_millis)
    } else if s.ends_with('s') {
        s.trim_end_matches('s').parse::<u64>().ok().map(Duration::from_secs)
    } else if s.ends_with('m') {
        s.trim_end_matches('m').parse::<u64>().ok().map(|m| Duration::from_secs(m * 60))
    } else {
        s.parse::<u64>().ok().map(Duration::from_millis)
    }
}

pub struct Engine {
    shared_data: crate::bridge::data::SharedData,
}

pub mod distributed;
pub mod http_client;
pub mod control;
pub mod io_bridge;

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

    pub fn extract_config(&self, script_path: PathBuf, script_content: String) -> Result<Option<Config>> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let dummy_shared_data = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
        let dummy_aggregator = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));
        let tokio_rt = Arc::new(tokio::runtime::Builder::new_current_thread().enable_all().build()?);
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        let config = context.with(|ctx| {
            crate::bridge::register_globals_sync(&ctx, tx, client, dummy_shared_data, 0, dummy_aggregator, tokio_rt, None, None).expect("Failed to register globals in config extraction");
            let module_name = script_path.to_string_lossy().to_string();
            let module = Module::declare(ctx.clone(), module_name, script_content)?;
            let (module, _) = module.eval()?;
            if let Ok(options) = module.get::<_, Value>("options") {
                if options.is_object() {
                    let json_str = json_stringify(ctx.clone(), options)?;
                    let mut config_val: serde_json::Value = serde_json::from_str(&json_str)?;
                    if let Some(obj) = config_val.as_object_mut() {
                        if let Some(s) = obj.remove("stages") { obj.insert("schedule".to_string(), s); }
                        if let Some(t) = obj.remove("thresholds") { obj.insert("criteria".to_string(), t); }
                        if let Some(e) = obj.remove("executor") { obj.insert("executor".to_string(), e); }
                        if let Some(r) = obj.remove("rate") { obj.insert("rate".to_string(), r); }
                        if let Some(tu) = obj.remove("timeUnit") { obj.insert("time_unit".to_string(), tu); }
                        if let Some(mid) = obj.remove("minIterationDuration") { obj.insert("min_iteration_duration".to_string(), mid); }
                        
                        // Handle scenarios with k6-compatible key mapping
                        if let Some(scenarios) = obj.get_mut("scenarios") {
                            if let Some(scenarios_obj) = scenarios.as_object_mut() {
                                for (_name, scenario) in scenarios_obj.iter_mut() {
                                    if let Some(scenario_obj) = scenario.as_object_mut() {
                                        // Map k6 keys to Thruster keys
                                        if let Some(vus) = scenario_obj.remove("vus") { scenario_obj.insert("workers".to_string(), vus); }
                                        if let Some(stages) = scenario_obj.remove("stages") { scenario_obj.insert("schedule".to_string(), stages); }
                                        if let Some(tu) = scenario_obj.remove("timeUnit") { scenario_obj.insert("time_unit".to_string(), tu); }
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
        // Proper cleanup: GC first, then drop context before runtime
        runtime.run_gc();
        drop(context);
        drop(runtime);
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
        _export_html: Option<PathBuf>,
        controller_metrics_url: Option<String>,
        control_rx: Option<std::sync::mpsc::Receiver<control::ControlCommand>>,
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

// Initialize control state early
        let control_state = Arc::new(control::ControlState::new(config.workers.unwrap_or(1)));
        
        // 1. Run Setup (same as before)
        let setup_data = self.run_setup(&script_path, &script_content, Arc::new(std::sync::RwLock::new(StatsAggregator::new())))
            .unwrap_or_else(|e| {
                eprintln!("Setup failed: {}", e);
                None
            });

        let setup_data = Arc::new(setup_data);

        let handle = std::thread::spawn(move || {
            // Calculate total workers for scaling decisions
            let total_workers = if let Some(ref scenarios) = config.scenarios {
                scenarios.values().map(|s| s.workers.unwrap_or(1)).sum::<usize>()
            } else {
                config.workers.unwrap_or(1)
            };

            // Dynamic shard count: target ~100 workers per shard for optimal contention
            // At 10k workers: 100 shards = 100 workers/shard (vs old 625 workers/shard)
            let num_shards = (total_workers / 100).max(16).min(256);
            let sharded_aggregator = Arc::new(ShardedAggregator::new(num_shards));

            // Use crossbeam bounded channel for backpressure at extreme load
            // Scale buffer with workers, with higher limits for extreme concurrency
            let channel_size = if total_workers > 30000 {
                (total_workers * 5).clamp(50_000, 250_000)
            } else {
                (total_workers * 10).clamp(20_000, 100_000)
            };
            let (tx, rx): (Sender<Metric>, Receiver<Metric>) = crossbeam_channel::bounded(channel_size);

            let agg_handle = sharded_aggregator.clone();
            let metrics_url = controller_metrics_url.clone();

            // Spawn multiple aggregator consumer threads for parallel metric processing
            // This prevents the single-consumer bottleneck at high throughput
            let num_aggregators = num_cpus::get().max(2) / 2;
            for agg_id in 0..num_aggregators {
                let rx = rx.clone();
                let agg_handle = agg_handle.clone();
                let metrics_url = metrics_url.clone();
                let num_shards = num_shards;

                std::thread::spawn(move || {
                    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                    let client = if metrics_url.is_some() {
                        let _guard = rt.enter();
                        Some(HttpClient::new())
                    } else { None };

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
                                if last_send.elapsed() >= Duration::from_secs(1) || batch.len() >= 100 {
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
                                        let _ = rt.block_on(client.request(req));
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

            // Scale Tokio threads more aggressively for better latency at high concurrency
            // Target: ~75 workers per Tokio thread (vs old ~150)
            // At 10k workers: 128 threads for lower task queue latency
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
                    .expect("Failed to create shared Tokio runtime")
            );
            // Scale connection pool with workers: target ~1 idle connection per 5 workers
            // At 10k workers: 2000 idle connections per host (reduced from 3333)
            let pool_size = (total_workers / 5).max(500).min(2000);
            let shared_http_client = {
                let _guard = shared_tokio_rt.enter();
                HttpClient::with_pool_and_workers(pool_size, total_workers)
            };

            // Configure may green thread scheduler with fixed stack size
            // 16KB works for 50K+ workers and complex scripts. Override via options.stack_size if needed.
            let may_stack_size = 16 * 1024;
            may::config()
                .set_workers(num_cpus::get())
                .set_stack_size(may_stack_size);

            // Create I/O bridge for may coroutines to access Tokio HTTP
            let num_io_workers = tokio_threads.min(32); // I/O workers match Tokio threads
            let _io_bridge = Arc::new(IoBridge::new(
                shared_tokio_rt.clone(),
                Arc::new(shared_http_client.clone()),
                num_io_workers,
            ));

            // eprintln!("[Engine] Workers: {}, Tokio threads: {}, Shards: {}, Aggregators: {}, Pool: {}, May stack: {}KB, I/O workers: {}",
            //          total_workers, tokio_threads, num_shards, num_aggregators, pool_size, may_stack_size / 1024, num_io_workers);

            let start_time = Instant::now();
            let mut active_scenario_handles: Vec<JoinHandle<()>> = Vec::new(); // For multi-scenario
            let mut active_legacy_workers: Vec<(may::coroutine::JoinHandle<()>, Arc<AtomicBool>)> = Vec::new(); // For legacy (green threads)
            let mut dynamic_target_legacy: Option<usize> = None; // For legacy ramp
            let schedule_legacy: Vec<(Duration, usize)>;
            let duration_legacy: Duration;
            let mut next_worker_id = 1;

            // Check if we have multiple scenarios
            let is_multi_scenario = config.scenarios.is_some();

            if let Some(ref scenarios) = config.scenarios {
                // Multi-scenario mode: spawn a worker pool per scenario
                
                // Duration for the loop is the max duration of all scenarios
                // Actually we just wait for handles to finish.
                
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
                    let drop = config.drop;
                    let shared_tokio_rt = shared_tokio_rt.clone();
                    let shared_http_client = shared_http_client.clone();
                    let control_state = control_state.clone();
                    
                    let h = std::thread::spawn(move || {
                        // Handle startTime delay
                        if let Some(ref st) = scenario_config.start_time {
                            if let Some(delay) = parse_duration_str(st) {
                                std::thread::sleep(delay);
                            }
                        }
                        
                        let exec_fn = scenario_config.exec.clone().unwrap_or_else(|| "default".to_string());
                        let duration = scenario_config.duration.as_deref().unwrap_or("10s");
                        let scenario_duration = parse_duration(duration).unwrap_or(Duration::from_secs(10));
                        let workers = scenario_config.workers.unwrap_or(1);
                        let max_iterations = scenario_config.iterations;
                        
                        println!("[Scenario: {}] Starting {} workers for {}...", scenario_name, workers, duration);
                        
                        let start_time = Instant::now();
                        let mut active_workers: Vec<(may::coroutine::JoinHandle<()>, Arc<AtomicBool>)> = Vec::new();
                        let mut next_worker_id = 1;
                        
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
                            let drop = drop;
                            // Use worker-scaled stack for green threads (configurable)
                            let stack_sz = scenario_config.stack_size.unwrap_or(may_stack_size);
                            let tokio_rt = shared_tokio_rt.clone();
                            let client = shared_http_client.clone();
                            let control_state = control_state.clone();

                            // Spawn as may coroutine instead of OS thread
                            // SAFETY: Stack size is configured to be sufficient for JS execution
                            let worker_handle = unsafe { may::coroutine::Builder::new()
                                .stack_size(stack_sz)
                                .spawn(move || {
                                let (runtime, context) = Self::create_runtime().unwrap();
                                runtime.set_memory_limit(256 * 1024); // 256KB per worker max JS heap
                                context.with(|ctx| {
                                    // Use standard HTTP path with hyper/Tokio (better connection pooling)
                                    crate::bridge::register_globals_sync(&ctx, tx.clone(), client, shared_data, worker_id, agg, tokio_rt, jitter, drop).unwrap();
                                    // Set scenario name global for automatic tagging
                                    ctx.globals().set("__SCENARIO", scenario_name.clone()).unwrap();
                                    let module = Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content).unwrap();
                                    let (module, _) = module.eval().unwrap();
                                    
                                    // Get the exec function (default or custom)
                                    let func: Function = module.get(&exec_fn).unwrap_or_else(|_| {
                                        eprintln!("[Scenario: {}] Function '{}' not found, using 'default'", scenario_name, exec_fn);
                                        module.get("default").unwrap()
                                    });
                                    
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

                                        let iter_start = Instant::now();
                                        let call_result = func.call::<_, ()>((data_val.clone(),)).catch(&ctx);
                                        let elapsed = iter_start.elapsed(); // Single elapsed() call
                                        let timings = crate::stats::RequestTimings { duration: elapsed, ..Default::default() };
                                        // Get tags Arc once (no clone of HashMap contents)
                                        let tags_arc = control_state.get_tags();
                                        let tags: HashMap<String, String> = (*tags_arc).clone();

                                        match call_result {
                                            Ok(_) => {
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration", scenario_name),
                                                    timings,
                                                    status: 200,
                                                    error: None,
                                                    tags,
                                                });
                                            },
                                            Err(e) => {
                                                let _ = tx.send(Metric::Request {
                                                    name: format!("{}::iteration", scenario_name),
                                                    timings,
                                                    status: 0,
                                                    error: Some(e.to_string()),
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
                                                may::coroutine::sleep(min_dur - elapsed);
                                            }
                                        }
                                        // No artificial delay - let async I/O provide natural pacing
                                        // This allows maximum throughput when min_iteration_duration is not set
                                    }
                                });
                                // Proper cleanup: GC first, then drop context before runtime
                                runtime.run_gc();
                                std::mem::drop(context);
                                std::mem::drop(runtime);
                            }) };
                            active_workers.push((worker_handle.expect("Failed to spawn coroutine"), running));
                        }
                        
                        // Wait for scenario duration
                        while start_time.elapsed() < scenario_duration {
                             // Also check global stop
                            if control_state.is_stopped() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        
                        // Stop all workers
                        for (_, r) in active_workers.iter() { r.store(false, Ordering::Relaxed); }
                        
                        // Wait for workers to finish
                        for (h, _) in active_workers {
                            let _ = h.join();
                        }
                        
                        println!("[Scenario: {}] Complete.", scenario_name);
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
                    for step in s { parsed.push((parse_duration(&step.duration).unwrap(), step.target)); }
                    parsed
                } else {
                    let duration_str = config.duration.as_deref().unwrap_or("10s");
                    let workers = config.workers.unwrap_or(1);
                    let duration = parse_duration(duration_str).unwrap();
                    vec![(Duration::from_secs(0), workers), (duration, workers)]
                };

                duration_legacy = schedule_legacy.iter().map(|(d, _)| *d).sum();
            }

            // UNIFIED CONTROL LOOP
            
            // If multi-scenario, we loop until all handles are done or stopped
            // If legacy, we loop based on duration_legacy
            
            'main_loop: loop {
                if is_multi_scenario {
                    if active_scenario_handles.iter().all(|h| h.is_finished()) {
                        break 'main_loop;
                    }
                } else if start_time.elapsed() >= duration_legacy && !control_state.is_stopped() { 
                    // Only break if natural finish
                    break 'main_loop;
                }
                
                if control_state.is_stopped() {
                    break 'main_loop;
                }

                // Process control commands
                if let Some(ref rx) = control_rx {
                    while let Ok(cmd) = rx.try_recv() {
                        match cmd {
                            control::ControlCommand::Ramp(n) => {
                                if is_multi_scenario {
                                    println!("[Control] WARNING: Ramping not supported in multi-scenario mode.");
                                } else {
                                    println!("[Control] Ramping to {} workers", n);
                                    dynamic_target_legacy = Some(n);
                                }
                            }
                            control::ControlCommand::Pause => {
                                println!("[Control] Pausing workers...");
                                control_state.pause();
                            }
                            control::ControlCommand::Resume => {
                                println!("[Control] Resuming workers...");
                                control_state.resume();
                            }
                            control::ControlCommand::Tag(k, v) => {
                                println!("[Control] Adding tag: {}={}", k, v);
                                control_state.add_tag(k, v);
                            }
                            control::ControlCommand::Status => {
                                let paused = if control_state.is_paused() { "PAUSED" } else { "RUNNING" };
                                let vus = if is_multi_scenario {
                                    // We don't verify exact VUs here easily without atomic counting, just printed status
                                    "many".to_string() 
                                } else {
                                    active_legacy_workers.len().to_string()
                                };
                                println!("[Status] Workers: {}, State: {}", vus, paused);
                            }
                            control::ControlCommand::Stop => {
                                println!("[Control] Stopping test...");
                                control_state.stop();
                                break 'main_loop;
                            }
                        }
                    }
                }

                if !is_multi_scenario && !control_state.is_stopped() {
                    // Legacy scaling logic
                    let elapsed = start_time.elapsed();
                    let scheduled_target = Self::calculate_target(&schedule_legacy, elapsed);
                    let target_workers = dynamic_target_legacy.unwrap_or(scheduled_target);

                    if target_workers > active_legacy_workers.len() {
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
                            let drop = config.drop;
                            let control_state = control_state.clone();
                            // Legacy mode: use tiered stack sizes same as multi-scenario
                            let stack_sz = config.stack_size.unwrap_or(may_stack_size);
                            let tokio_rt = shared_tokio_rt.clone();
                            let client = shared_http_client.clone();
                            
                            // Spawn as may coroutine instead of OS thread for higher concurrency
                            // SAFETY: Stack size is configured to be sufficient for JS execution
                            let h = unsafe { may::coroutine::Builder::new()
                                .stack_size(stack_sz)
                                .spawn(move || {
                                let (runtime, context) = Self::create_runtime().unwrap();
                                runtime.set_memory_limit(256 * 1024); // 256KB per worker max JS heap
                                context.with(|ctx| {
                                    // Pass std::sync::mpsc::Sender directly - no bridge thread needed!
                                    // Clone tx since we need it for both bridge and iteration metrics
                                    crate::bridge::register_globals_sync(&ctx, tx.clone(), client, shared_data, worker_id, agg, tokio_rt, jitter, drop).unwrap();
                                    let module = Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content).unwrap();
                                    let (module, _) = module.eval().unwrap();
                                    let func: Function = module.get("default").unwrap();
                                    
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
                                        
                                        let iter_start = Instant::now();
                                        let call_result = func.call::<_, ()>((data_val.clone(),)).catch(&ctx);
                                        let elapsed = iter_start.elapsed(); // Single elapsed() call
                                        let timings = crate::stats::RequestTimings { duration: elapsed, ..Default::default() };
                                        // Get tags Arc once (no clone of HashMap contents)
                                        let tags_arc = control_state.get_tags();
                                        let tags: HashMap<String, String> = (*tags_arc).clone();

                                        match call_result {
                                            Ok(_) => {
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration".to_string(),
                                                    timings,
                                                    status: 200,
                                                    error: None,
                                                    tags,
                                                });
                                            },
                                            Err(e) => {
                                                let _ = tx.send(Metric::Request {
                                                    name: "iteration".to_string(),
                                                    timings,
                                                    status: 0,
                                                    error: Some(e.to_string()),
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
                                // Proper cleanup: GC first, then drop context before runtime
                                runtime.run_gc();
                                std::mem::drop(context);
                                std::mem::drop(runtime);
                            }) };
                            active_legacy_workers.push((h.expect("Failed to spawn coroutine"), running));
                        }
                    }
                    
                    // Scale down if target is less than current (only from ramp command)
                    if let Some(target) = dynamic_target_legacy {
                        while target < active_legacy_workers.len() && !active_legacy_workers.is_empty() {
                            if let Some((_, running)) = active_legacy_workers.pop() {
                                running.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                } // End legacy scaling for this loop tick
                
                std::thread::sleep(Duration::from_millis(100));
            } // End main_loop
            
            // Clean up
            if is_multi_scenario {
                // Wait for all scenarios to complete
                for h in active_scenario_handles {
                    let _ = h.join();
                }
            } else {
                for (_, r) in active_legacy_workers.iter() { r.store(false, Ordering::Relaxed); }
                // Graceful Stop for legacy
                let graceful_stop_duration = config.stop.as_deref()
                    .and_then(parse_duration_str)
                    .unwrap_or(Duration::from_secs(30));

                if graceful_stop_duration > Duration::ZERO {
                    println!("Graceful stop: waiting up to {}s for workers to finish...", graceful_stop_duration.as_secs_f64());
                    let stop_start = Instant::now();
                    while stop_start.elapsed() < graceful_stop_duration {
                         // Check if all workers have stopped (running flag is false means iteration loop exited)
                         let all_stopped = active_legacy_workers.iter().all(|(_, r)| !r.load(Ordering::Relaxed));
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

            if json_output { println!("{}", merged_agg.to_json()); }
            else { merged_agg.report(); }

            if let Some(path) = export_json { let _ = std::fs::write(path, merged_agg.to_json()); }

            // Save to history DB
            if let Ok(db) = crate::stats::db::HistoryDb::open_default() {
                let _ = db.save_run(&script_path.to_string_lossy(), &report);
            }

            // Teardown
            let _ = self.run_teardown(&script_path, &script_content, setup_data.as_deref().map(|s| s.to_string()), aggregator.clone());
            
            report
        });

        handle.join().map_err(|_| anyhow::anyhow!("Test thread panicked"))
    }

    fn calculate_target(schedule: &[(Duration, usize)], elapsed: Duration) -> usize {
        let mut active_time = Duration::from_secs(0);
        let mut prev_target = 0;
        for (duration, target) in schedule {
            if elapsed < active_time + *duration {
                let progress = (elapsed.as_secs_f64() - active_time.as_secs_f64()) / duration.as_secs_f64();
                let diff = *target as f64 - prev_target as f64;
                return (prev_target as f64 + diff * progress) as usize;
            }
            active_time += *duration;
            prev_target = *target;
        }
        0
    }

    pub fn warmup(&self, url: &str) {
        let url = url.to_string();
        // Spawn a few threads to establish multiple connections in the pool
        let threads: Vec<_> = (0..10).map(|_| {
            let u = url.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                let client = {
                    let _guard = rt.enter();
                    HttpClient::new()
                };
                let req = http::Request::builder()
                    .method(Method::HEAD)
                    .uri(&u)
                    .body("".to_string());
                
                if let Ok(r) = req {
                        let _ = rt.block_on(async move { tokio::time::timeout(Duration::from_secs(10), client.request(r)).await });
                }
            })
        }).collect();

        for t in threads {
            let _ = t.join();
        }
    }

    fn run_setup(&self, script_path: &Path, script_content: &str, aggregator: SharedAggregator) -> Result<Option<String>> {
        let (runtime, context) = Self::create_runtime()?;
        let (_tx, _rx) = crossbeam_channel::unbounded::<Metric>(); // Drop metrics from setup
        // We probably want to print them.
        let (tx_print, rx_print) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Metric::Log { message }) = rx_print.recv() {
                println!("{}", message);
            }
        });

        let shared_data = self.shared_data.clone();
        let tokio_rt = Arc::new(tokio::runtime::Builder::new_current_thread().enable_all().build()?);
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };
        
        let result = context.with(|ctx| {
            crate::bridge::register_globals_sync(&ctx, tx_print, client, shared_data, 0, aggregator, tokio_rt, None, None)?;
            let module = Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content)?;
            let (module, _) = module.eval()?;
            
            if let Ok(setup_fn) = module.get::<_, Function>("setup") {
                let result = setup_fn.call::<_, Value>(())?;
                if !result.is_undefined() {
                    return Ok(Some(json_stringify(ctx.clone(), result)?));
                }
            }
            Ok(None)
        });

        // Proper cleanup: GC first, then drop context before runtime
        runtime.run_gc();
        drop(context);
        drop(runtime);
        result
    }

    pub fn run_script(&self, script_content: &str) -> Result<()> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx, _rx) = crossbeam_channel::unbounded::<Metric>();
        let shared_data = self.shared_data.clone();
        let aggregator = Arc::new(std::sync::RwLock::new(StatsAggregator::new()));
        let tokio_rt = Arc::new(tokio::runtime::Builder::new_current_thread().enable_all().build()?);
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        context.with(|ctx| {
            crate::bridge::register_globals_sync(&ctx, tx, client, shared_data, 0, aggregator, tokio_rt, None, None)?;
            let _ = ctx.eval::<Value, _>(script_content)?;
            Ok::<(), anyhow::Error>(())
        })?;

        // Proper cleanup: GC first, then drop context before runtime
        runtime.run_gc();
        drop(context);
        drop(runtime);
        Ok(())
    }

    fn run_teardown(&self, script_path: &Path, script_content: &str, data: Option<String>, aggregator: SharedAggregator) -> Result<()> {
        let (runtime, context) = Self::create_runtime()?;
        let (tx_print, rx_print) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            while let Ok(Metric::Log { message }) = rx_print.recv() {
                println!("{}", message);
            }
        });

        let shared_data = self.shared_data.clone();
        let tokio_rt = Arc::new(tokio::runtime::Builder::new_current_thread().enable_all().build()?);
        let client = {
            let _guard = tokio_rt.enter();
            HttpClient::new()
        };

        let _ = context.with(|ctx| {
            crate::bridge::register_globals_sync(&ctx, tx_print, client, shared_data, 0, aggregator, tokio_rt, None, None)?;
            let module = Module::declare(ctx.clone(), script_path.to_string_lossy().to_string(), script_content)?;
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

        // Proper cleanup: GC first, then drop context before runtime
        runtime.run_gc();
        drop(context);
        drop(runtime);
        Ok(())
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
}