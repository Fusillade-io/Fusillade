use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

use fusillade::engine::distributed::{ControllerServer, WorkerServer};
use fusillade::engine::Engine;
use fusillade::parse_duration_str;

/// Parse drop probability with range validation (0.0-1.0)
fn parse_drop_probability(s: &str) -> Result<f64, String> {
    let val: f64 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid number", s))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!(
            "drop probability must be between 0.0 and 1.0, got {}",
            val
        ));
    }
    Ok(val)
}

/// Run a test on Fusillade Cloud
fn run_cloud_test(
    scenario: PathBuf,
    workers: Option<usize>,
    duration: Option<String>,
    region: String,
) -> Result<()> {
    // Check if logged in
    let auth = match fusillade::cli::cloud::load_token() {
        Some(a) => a,
        None => {
            eprintln!("Error: Not logged in to Fusillade Cloud.");
            eprintln!("Run 'fusillade login <token>' first.");
            eprintln!("Get your API key at https://fusillade.io/settings");
            return Ok(());
        }
    };

    // Read script content
    let script_content = std::fs::read_to_string(&scenario)
        .map_err(|e| anyhow::anyhow!("Failed to read script: {}", e))?;

    let script_name = scenario
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("script.js")
        .to_string();

    println!("Uploading test to Fusillade Cloud...");
    println!("  Script: {}", script_name);
    println!("  Workers: {}", workers.unwrap_or(10));
    println!("  Duration: {}", duration.as_deref().unwrap_or("60s"));
    println!("  Region: {}", region);

    // Build request
    let api_url = fusillade::cli::cloud::get_api_url();
    let client = reqwest::blocking::Client::new();

    let form = reqwest::blocking::multipart::Form::new()
        .text("name", script_name.clone())
        .text("vus", workers.unwrap_or(10).to_string())
        .text(
            "duration",
            parse_duration_str(duration.as_deref().unwrap_or("60s"))
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|| "60".to_string()),
        )
        .text("region", region)
        .text("script", script_content);

    let response = client
        .post(format!("{}/api/v1/tests", api_url))
        .header("Authorization", format!("Bearer {}", auth.token))
        .multipart(form)
        .send();

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().unwrap_or_default();
            let test_id = body
                .get("test_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            println!("\n✓ Test submitted successfully!");
            println!("  Test ID: {}", test_id);
            println!("  Status: pending");
            println!(
                "\nView results at: https://fusillade.io/dashboard/runs/{}",
                test_id
            );
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            eprintln!("Error: Cloud API returned {}", status);
            eprintln!("  {}", body);
        }
        Err(e) => {
            eprintln!("Error: Failed to connect to Fusillade Cloud");
            eprintln!("  {}", e);
            eprintln!("\nIs the control plane running? Check https://status.fusillade.io");
        }
    }

    Ok(())
}

#[derive(Parser)]
#[command(name = "fusillade")]
#[command(about = "High-performance load testing engine in Rust", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    Run {
        scenario: PathBuf,
        #[arg(short, long, alias = "vus")]
        workers: Option<usize>,
        #[arg(short, long)]
        duration: Option<String>,
        /// External configuration file (YAML or JSON)
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Run in headless mode (no TUI, suitable for CI/CD)
        #[arg(long)]
        headless: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        export_json: Option<PathBuf>,
        #[arg(long)]
        export_html: Option<PathBuf>,
        /// Output configuration (e.g., --out otlp=http://localhost:4318/v1/metrics)
        #[arg(long)]
        out: Option<String>,
        /// URL to stream real-time metrics to during test execution (e.g., http://localhost:8080/metrics)
        #[arg(long)]
        metrics_url: Option<String>,
        /// Authentication header for metrics URL (format: "HeaderName: value")
        #[arg(long)]
        metrics_auth: Option<String>,
        /// Chaos: Add jitter latency (e.g., "500ms")
        #[arg(long)]
        jitter: Option<String>,
        /// Chaos: Drop requests with probability (0.0-1.0)
        #[arg(long, value_parser = parse_drop_probability)]
        drop: Option<f64>,
        /// Estimate bandwidth and cost before running. Optionally set warning threshold in dollars (default: 10)
        #[arg(long)]
        estimate_cost: Option<Option<f64>>,
        /// Enable interactive control mode (pause, resume, ramp workers)
        #[arg(long, short = 'i')]
        interactive: bool,
        /// Run on Fusillade Cloud instead of locally
        #[arg(long)]
        cloud: bool,
        /// Cloud region to run in (default: us-east-1)
        #[arg(long, default_value = "us-east-1")]
        region: String,
        /// Disable per-endpoint (per-URL) metrics tracking
        #[arg(long)]
        no_endpoint_tracking: bool,
        /// Watch script for changes and re-run automatically (development mode)
        #[arg(long)]
        watch: bool,
        /// Disable pre-flight memory capacity check
        #[arg(long)]
        no_memory_check: bool,
        /// Enable memory-safe mode: throttle worker spawning if memory usage is high
        #[arg(long)]
        memory_safe: bool,
        /// Save test results to local history database
        #[arg(long)]
        save_history: bool,
        /// Capture failed requests to file for later replay (default: fusillade-errors.json)
        #[arg(long)]
        capture_errors: Option<Option<PathBuf>>,
        /// Fixed iterations per worker
        #[arg(long)]
        iterations: Option<u64>,
        /// Warmup URL to hit before test starts
        #[arg(long)]
        warmup: Option<String>,
        /// Discard response bodies to save memory
        #[arg(long)]
        response_sink: bool,
        /// Threshold criterion (can be repeated: --threshold 'http_req_duration:p95<500')
        #[arg(long = "threshold", value_name = "METRIC:CONDITION")]
        thresholds: Vec<String>,
        /// Abort test if any threshold fails
        #[arg(long)]
        abort_on_fail: bool,
        /// Log level: debug, info, warn, error
        #[arg(long, default_value = "info")]
        log_level: String,
        /// Filter logs to a specific scenario (e.g., --log-filter scenario=login)
        #[arg(long)]
        log_filter: Option<String>,
        /// Validate script and show execution plan without running the test
        #[arg(long)]
        dry_run: bool,
        /// Skip TLS certificate verification (insecure, use for self-signed certs)
        #[arg(long)]
        insecure: bool,
        /// Maximum number of HTTP redirects to follow (0 to disable)
        #[arg(long, default_value = "10")]
        max_redirects: u32,
        /// Default User-Agent header for HTTP requests
        #[arg(long)]
        user_agent: Option<String>,
        /// Disable connection pooling (new connection per request, enables connection timing)
        #[arg(long)]
        no_pool: bool,
    },
    /// Initialize a new test script with starter template
    Init {
        /// Output file path (default: test.js)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Also create a fusillade.yaml config file
        #[arg(long)]
        config: bool,
    },
    /// Validate a script without running it
    Validate {
        /// Path to the scenario JS file
        scenario: PathBuf,
        /// Optional config file to validate
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Generate shell completions
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    Worker {
        #[arg(short, long, default_value = "0.0.0.0:8080")]
        listen: String,
        #[arg(short, long)]
        connect: Option<String>,
    },
    Controller {
        #[arg(short, long, default_value = "0.0.0.0:9000")]
        listen: String,
    },
    Exec {
        script: String,
    },
    Convert {
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Schema {
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Record {
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short, long, default_value_t = 8085)]
        port: u16,
    },
    /// Replay failed requests from an errors file
    Replay {
        /// Path to fusillade-errors.json file
        input: PathBuf,
        /// Run requests sequentially (default) or parallel
        #[arg(long)]
        parallel: bool,
    },
    /// Export failed requests to different formats (e.g., cURL)
    Export {
        /// Path to fusillade-errors.json file
        input: PathBuf,
        /// Output format: curl
        #[arg(long, default_value = "curl")]
        format: String,
        /// Output file (stdout if not specified)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Authenticate with Fusillade Cloud using an API key
    Login {
        /// API key from https://fusillade.io/settings
        token: String,
    },
    /// Log out from Fusillade Cloud
    Logout,
    /// Check authentication status
    Whoami,
    /// Show version and status information
    Status,
    /// View test run history
    History {
        /// Show detailed report for a specific run ID
        #[arg(long)]
        show: Option<i64>,
        /// Number of recent runs to list (default: 10)
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Compare two test run summaries
    Compare {
        /// Path to baseline JSON summary
        baseline: PathBuf,
        /// Path to current JSON summary
        current: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            scenario,
            workers,
            duration,
            config,
            headless,
            json,
            export_json,
            export_html,
            out,
            metrics_url,
            metrics_auth,
            jitter,
            drop,
            estimate_cost,
            interactive,
            cloud,
            region,
            no_endpoint_tracking,
            watch,
            no_memory_check,
            memory_safe,
            save_history,
            capture_errors,
            iterations,
            warmup,
            response_sink,
            thresholds,
            abort_on_fail,
            log_level,
            log_filter,
            dry_run,
            insecure,
            max_redirects,
            user_agent,
            no_pool,
        } => {
            // Set log level for console.* API
            let log_level_enum = match log_level.to_lowercase().as_str() {
                "debug" => fusillade::bridge::LogLevel::Debug,
                "warn" | "warning" => fusillade::bridge::LogLevel::Warn,
                "error" => fusillade::bridge::LogLevel::Error,
                _ => fusillade::bridge::LogLevel::Info,
            };
            fusillade::bridge::set_log_level(log_level_enum);

            // Set log filter if specified (e.g., --log-filter scenario=login)
            if let Some(ref filter) = log_filter {
                fusillade::bridge::set_log_filter(filter.clone());
            }

            // Load .env file if present (check script directory, then current directory)
            let script_dir = scenario.parent().unwrap_or(std::path::Path::new("."));
            let env_paths = [script_dir.join(".env"), PathBuf::from(".env")];
            for env_path in &env_paths {
                if env_path.exists() {
                    if let Ok(contents) = std::fs::read_to_string(env_path) {
                        for line in contents.lines() {
                            let line = line.trim();
                            if line.is_empty() || line.starts_with('#') {
                                continue;
                            }
                            if let Some((key, value)) = line.split_once('=') {
                                let key = key.trim();
                                let value = value.trim().trim_matches('"').trim_matches('\'');
                                if std::env::var(key).is_err() {
                                    std::env::set_var(key, value);
                                }
                            }
                        }
                    }
                    break; // Only load first .env found
                }
            }

            // Cloud mode: Upload to Fusillade Cloud
            if cloud {
                return run_cloud_test(scenario, workers, duration, region);
            }

            // Watch mode: Automatically re-run test when script changes
            if watch {
                println!("Watch mode: monitoring {} for changes", scenario.display());
                println!("Press Ctrl+C to stop\n");

                let mut last_modified =
                    std::fs::metadata(&scenario).and_then(|m| m.modified()).ok();

                loop {
                    // Run test with reduced settings for dev workflow
                    let engine = Engine::new()?;
                    let script_content = std::fs::read_to_string(&scenario).unwrap();
                    let mut watch_config = engine
                        .extract_config(scenario.clone(), script_content.clone())
                        .unwrap()
                        .unwrap_or_default();

                    // Use provided values or defaults for watch mode
                    if let Some(w) = workers {
                        watch_config.workers = Some(w);
                    } else if watch_config.workers.is_none() {
                        watch_config.workers = Some(1);
                    }

                    if let Some(d) = duration.clone() {
                        watch_config.duration = Some(d);
                    } else if watch_config.duration.is_none() {
                        watch_config.duration = Some("5s".to_string());
                    }

                    if let Some(j) = jitter.clone() {
                        watch_config.jitter = Some(j);
                    }
                    if let Some(p) = drop {
                        watch_config.drop = Some(p);
                    }
                    if no_endpoint_tracking {
                        watch_config.no_endpoint_tracking = Some(true);
                    }

                    println!(
                        "--- Running test (workers: {}, duration: {}) ---",
                        watch_config.workers.unwrap_or(1),
                        watch_config.duration.as_deref().unwrap_or("5s")
                    );

                    let engine_arc = Arc::new(engine);
                    match engine_arc.run_load_test(
                        scenario.clone(),
                        script_content,
                        watch_config,
                        true, // Always headless in watch mode
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                        None,
                    ) {
                        Ok(_) => {}
                        Err(e) => eprintln!("[Watch] Test failed: {}", e),
                    }

                    println!("\n--- Waiting for changes to {} ---\n", scenario.display());

                    // Poll for file changes
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        let current_modified =
                            std::fs::metadata(&scenario).and_then(|m| m.modified()).ok();

                        if current_modified != last_modified {
                            last_modified = current_modified;
                            println!("File changed, re-running...\n");
                            break;
                        }
                    }
                }
            }

            // Local mode: Run test locally
            let engine = Engine::new()?;
            let script_content = std::fs::read_to_string(&scenario).unwrap();

            // Start with config from script's export const options
            let mut final_config = engine
                .extract_config(scenario.clone(), script_content.clone())
                .unwrap()
                .unwrap_or_default();

            // Merge external config file if provided (overrides script config)
            if let Some(config_path) = config {
                let config_content = std::fs::read_to_string(&config_path)
                    .map_err(|e| anyhow::anyhow!("Failed to read config file: {}", e))?;

                let file_config: fusillade::cli::config::Config = if config_path
                    .extension()
                    .map(|e| e == "json")
                    .unwrap_or(false)
                {
                    serde_json::from_str(&config_content)
                        .map_err(|e| anyhow::anyhow!("Invalid JSON config: {}", e))?
                } else {
                    serde_yaml::from_str(&config_content)
                        .map_err(|e| anyhow::anyhow!("Invalid YAML config: {}", e))?
                };

                // Merge file config into final_config (file overrides script)
                if file_config.workers.is_some() {
                    final_config.workers = file_config.workers;
                }
                if file_config.duration.is_some() {
                    final_config.duration = file_config.duration;
                }
                if file_config.schedule.is_some() {
                    final_config.schedule = file_config.schedule;
                }
                if file_config.executor.is_some() {
                    final_config.executor = file_config.executor;
                }
                if file_config.rate.is_some() {
                    final_config.rate = file_config.rate;
                }
                if file_config.time_unit.is_some() {
                    final_config.time_unit = file_config.time_unit;
                }
                if file_config.criteria.is_some() {
                    final_config.criteria = file_config.criteria;
                }
                if file_config.min_iteration_duration.is_some() {
                    final_config.min_iteration_duration = file_config.min_iteration_duration;
                }
                if file_config.warmup.is_some() {
                    final_config.warmup = file_config.warmup;
                }
                if file_config.stop.is_some() {
                    final_config.stop = file_config.stop;
                }
                if file_config.iterations.is_some() {
                    final_config.iterations = file_config.iterations;
                }
                if file_config.scenarios.is_some() {
                    final_config.scenarios = file_config.scenarios;
                }
                if file_config.jitter.is_some() {
                    final_config.jitter = file_config.jitter;
                }
                if file_config.drop.is_some() {
                    final_config.drop = file_config.drop;
                }
                if file_config.stack_size.is_some() {
                    final_config.stack_size = file_config.stack_size;
                }
                if file_config.response_sink.is_some() {
                    final_config.response_sink = file_config.response_sink;
                }
                if file_config.no_endpoint_tracking.is_some() {
                    final_config.no_endpoint_tracking = file_config.no_endpoint_tracking;
                }
                if file_config.abort_on_fail.is_some() {
                    final_config.abort_on_fail = file_config.abort_on_fail;
                }
                if file_config.memory_safe.is_some() {
                    final_config.memory_safe = file_config.memory_safe;
                }
                if file_config.no_pool.is_some() {
                    final_config.no_pool = file_config.no_pool;
                }
            }

            // CLI flags override everything (highest priority)
            if let Some(w) = workers {
                final_config.workers = Some(w);
            }
            if let Some(d) = duration {
                final_config.duration = Some(d);
            }
            if let Some(j) = jitter {
                final_config.jitter = Some(j);
            }
            if let Some(p) = drop {
                final_config.drop = Some(p);
            }
            if no_endpoint_tracking {
                final_config.no_endpoint_tracking = Some(true);
            }
            if memory_safe {
                final_config.memory_safe = Some(true);
            }
            if let Some(iters) = iterations {
                final_config.iterations = Some(iters);
            }
            if let Some(ref w) = warmup {
                final_config.warmup = Some(w.clone());
            }
            if response_sink {
                final_config.response_sink = Some(true);
            }
            if abort_on_fail {
                final_config.abort_on_fail = Some(true);
            }
            if insecure {
                final_config.insecure = Some(true);
            }
            if no_pool {
                final_config.no_pool = Some(true);
            }
            final_config.max_redirects = Some(max_redirects);
            if let Some(ref ua) = user_agent {
                final_config.user_agent = Some(ua.clone());
            }
            // Parse --threshold flags into criteria map
            if !thresholds.is_empty() {
                let mut criteria = final_config.criteria.unwrap_or_default();
                for t in &thresholds {
                    // Parse "metric:condition" format
                    if let Some((metric, condition)) = t.split_once(':') {
                        if metric.is_empty() {
                            eprintln!(
                                "Warning: Ignoring threshold with empty metric name: '{}'",
                                t
                            );
                            continue;
                        }
                        if condition.is_empty() {
                            eprintln!("Warning: Ignoring threshold with empty condition: '{}'", t);
                            continue;
                        }
                        // Validate condition syntax (basic check)
                        if !condition.contains('<') && !condition.contains('>') {
                            eprintln!(
                                "Warning: Threshold condition '{}' may be invalid (missing < or >)",
                                condition
                            );
                        }
                        criteria
                            .entry(metric.to_string())
                            .or_default()
                            .push(condition.to_string());
                    } else {
                        eprintln!(
                            "Warning: Ignoring malformed threshold '{}' (expected format: METRIC:CONDITION)",
                            t
                        );
                    }
                }
                final_config.criteria = Some(criteria);
            }

            // Dry run: show execution plan and exit
            if dry_run {
                println!("╭────────────────────────────────────────────────────────────╮");
                println!("│ DRY RUN - Execution Plan                                  │");
                println!("├────────────────────────────────────────────────────────────┤");
                println!("│ Script:      {:>44} │", scenario.display());
                println!("│ Workers:     {:>44} │", final_config.workers.unwrap_or(1));
                println!(
                    "│ Duration:    {:>44} │",
                    final_config
                        .duration
                        .as_deref()
                        .unwrap_or("(iterations-based)")
                );
                if let Some(iters) = final_config.iterations {
                    println!("│ Iterations:  {:>44} │", iters);
                }
                if let Some(ref executor) = final_config.executor {
                    println!("│ Executor:    {:>44?} │", executor);
                }
                if let Some(ref rate) = final_config.rate {
                    println!("│ Rate:        {:>44} │", rate);
                }
                if let Some(ref schedule) = final_config.schedule {
                    println!("├────────────────────────────────────────────────────────────┤");
                    println!("│ Schedule Stages:                                           │");
                    for (i, stage) in schedule.iter().enumerate() {
                        println!(
                            "│   Stage {}: {:>3} workers for {:>37} │",
                            i + 1,
                            stage.target,
                            stage.duration
                        );
                    }
                }
                if let Some(ref scenarios) = final_config.scenarios {
                    println!("├────────────────────────────────────────────────────────────┤");
                    println!("│ Scenarios:                                                 │");
                    for (name, sc) in scenarios {
                        println!(
                            "│   {}: {:>3} workers                                     │",
                            name,
                            sc.workers.unwrap_or(1)
                        );
                    }
                }
                if let Some(ref criteria) = final_config.criteria {
                    println!("├────────────────────────────────────────────────────────────┤");
                    println!("│ Thresholds:                                                │");
                    for (metric, conditions) in criteria {
                        for condition in conditions {
                            println!(
                                "│   {}:{}                                               │",
                                metric, condition
                            );
                        }
                    }
                }
                println!("╰────────────────────────────────────────────────────────────╯");
                return Ok(());
            }

            // Pre-flight memory check (unless disabled)
            if !no_memory_check {
                let requested_workers = final_config.workers.unwrap_or(1);
                let preflight = fusillade::engine::memory::preflight_check(requested_workers);

                if !preflight.safe {
                    eprintln!("╭────────────────────────────────────────────────────────────╮");
                    eprintln!("│ MEMORY WARNING                                              │");
                    eprintln!("├────────────────────────────────────────────────────────────┤");
                    eprintln!(
                        "│ Requested workers: {:>8}                                │",
                        preflight.requested
                    );
                    eprintln!(
                        "│ Estimated max:     {:>8} (based on available RAM)       │",
                        preflight.estimated_max
                    );
                    eprintln!(
                        "│ Available RAM:     {:>8}                                │",
                        fusillade::engine::memory::format_bytes(preflight.available_bytes)
                    );
                    eprintln!(
                        "│ Estimated needed:  {:>8}                                │",
                        fusillade::engine::memory::format_bytes(preflight.estimated_needed)
                    );
                    eprintln!("├────────────────────────────────────────────────────────────┤");
                    eprintln!("│ The test may run out of memory and crash.                  │");
                    eprintln!("│ Use --no-memory-check to suppress this warning.            │");
                    if memory_safe {
                        eprintln!("│ Memory-safe mode enabled: will throttle if needed.         │");
                    }
                    eprintln!("╰────────────────────────────────────────────────────────────╯");
                    eprintln!();
                }
            }

            // Headless mode: either --headless or --json enables non-TUI mode
            let headless_mode = headless || json;
            let engine_arc = Arc::new(engine);

            // Cost estimation if requested
            if let Some(threshold_opt) = estimate_cost {
                let cost_threshold = threshold_opt.unwrap_or(10.0);
                println!("\nRunning cost estimation (dry run)...");
                let dry_config = fusillade::cli::config::Config {
                    workers: Some(1),
                    duration: Some("3s".to_string()),
                    iterations: Some(5),
                    ..final_config.clone()
                };
                let dry_report = engine_arc.clone().run_load_test(
                    scenario.clone(),
                    script_content.clone(),
                    dry_config,
                    true,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;

                // Calculate estimates
                let requests = dry_report.total_requests.max(1) as f64;
                let avg_response_bytes: f64 = dry_report
                    .grouped_requests
                    .values()
                    .map(|r| r.avg_receiving_ms * 1024.0) // Approximate: use receive time as proxy when size unavailable
                    .sum::<f64>()
                    / requests;

                // Estimate based on requested config
                let target_workers = final_config.workers.unwrap_or(1) as f64;
                let target_duration_secs =
                    parse_duration_str(final_config.duration.as_deref().unwrap_or("60s"))
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(60.0);
                let avg_req_duration_secs = dry_report.avg_latency_ms / 1000.0;
                let est_total_requests = if avg_req_duration_secs > 0.0 {
                    (target_workers * target_duration_secs / avg_req_duration_secs) as u64
                } else {
                    (target_workers * target_duration_secs * 10.0) as u64 // Fallback: 10 req/s per worker
                };

                // Assume ~10KB average response if we can't measure (conservative estimate)
                let est_bytes_per_req = if avg_response_bytes > 0.0 {
                    avg_response_bytes
                } else {
                    10_000.0
                };
                let est_total_bytes = est_total_requests as f64 * est_bytes_per_req * 2.0; // *2 for request+response
                let est_gb = est_total_bytes / 1_073_741_824.0;
                let est_cost = est_gb * 0.09; // AWS standard egress cost

                // Only show warning if estimated cost exceeds user-defined threshold
                if est_cost > cost_threshold {
                    println!(
                        "\nWARNING: Estimated cost exceeds ${:.0} threshold",
                        cost_threshold
                    );
                } else {
                    println!("\nCost Estimation");
                }
                println!("------------------------------------");
                println!("Est. Requests:      ~{}", est_total_requests);
                println!("Est. Data Transfer: {:.2} GB", est_gb);
                println!("Est. AWS Cost:      ~${:.2} (at $0.09/GB)", est_cost);
                println!("------------------------------------");

                // Only prompt if running interactively (TTY)
                if atty::is(atty::Stream::Stdin) {
                    print!("Proceed? [y/N] ");
                    use std::io::Write;
                    std::io::stdout().flush().unwrap();
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).unwrap();
                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Aborted.");
                        return Ok(());
                    }
                }
                println!("\nProceeding with full test...\n");
            }

            // Setup control channel and optional TUI
            let (control_tx, control_rx) =
                std::sync::mpsc::channel::<fusillade::engine::control::ControlCommand>();

            // Determine total duration for TUI progress display
            let tui_total_duration = if let Some(ref schedule) = final_config.schedule {
                let total: u64 = schedule
                    .iter()
                    .filter_map(|s| fusillade::parse_duration_str(&s.duration).map(|d| d.as_secs()))
                    .sum();
                Some(Duration::from_secs(total))
            } else {
                final_config
                    .duration
                    .as_deref()
                    .and_then(fusillade::parse_duration_str)
            };

            // Create shared arcs for TUI communication
            let initial_workers = final_config.workers.unwrap_or(1);
            let shared_control_state = Arc::new(fusillade::engine::control::ControlState::new(
                initial_workers,
            ));

            // Calculate shard count for shared aggregator (same formula as engine)
            let total_workers_est = if let Some(ref scenarios) = final_config.scenarios {
                scenarios
                    .values()
                    .map(|s| s.workers.unwrap_or(1))
                    .sum::<usize>()
            } else {
                initial_workers
            };
            let num_shards = (total_workers_est / 100).clamp(16, 256);
            let shared_aggregator = Arc::new(fusillade::stats::ShardedAggregator::new(num_shards));

            let tui_handle = if !headless_mode {
                let agg = shared_aggregator.clone();
                let cs = shared_control_state.clone();
                let tx = control_tx.clone();
                let dur = tui_total_duration;
                Some(std::thread::spawn(move || {
                    if let Err(e) = fusillade::tui::run_tui(agg, tx, cs, dur) {
                        eprintln!("TUI error: {}", e);
                    }
                }))
            } else {
                None
            };

            // In headless+interactive mode, use stdin reader
            if headless_mode && interactive {
                use fusillade::engine::control::parse_control_command;
                use std::io::BufRead;

                let tx = control_tx.clone();
                println!(
                    "Interactive mode enabled. Commands: ramp <N>, pause, resume, status, stop"
                );
                println!("   Type commands and press Enter.\n");

                std::thread::spawn(move || {
                    let stdin = std::io::stdin();
                    for line in stdin.lock().lines().map_while(Result::ok) {
                        if let Some(cmd) = parse_control_command(&line) {
                            if tx.send(cmd).is_err() {
                                break;
                            }
                        } else if !line.trim().is_empty() {
                            println!("Unknown command: {}", line.trim());
                        }
                    }
                });
            }

            let report = engine_arc.run_load_test(
                scenario.clone(),
                script_content,
                final_config,
                headless_mode,
                export_json,
                export_html,
                metrics_url,
                metrics_auth,
                Some(control_rx),
                Some(shared_aggregator),
                Some(shared_control_state),
            )?;

            // Wait for TUI thread to finish
            if let Some(handle) = tui_handle {
                let _ = handle.join();
            }

            // Handle OTLP export if --out otlp=<url> is specified
            if let Some(out_config) = out {
                if let Some(url) = out_config.strip_prefix("otlp=") {
                    println!("Exporting metrics to OTLP: {}", url);
                    let exporter = fusillade::stats::otlp::OtlpExporter::new(url);
                    if let Err(e) = exporter.export(&report) {
                        eprintln!("OTLP export failed: {}", e);
                    } else {
                        println!("OTLP export successful!");
                    }
                } else if let Some(path) = out_config.strip_prefix("csv=") {
                    println!("Exporting metrics to CSV: {}", path);
                    let csv_content = fusillade::stats::csv::generate_csv(&report);
                    if let Err(e) = std::fs::write(path, csv_content) {
                        eprintln!("CSV export failed: {}", e);
                    } else {
                        println!("CSV export successful!");
                    }
                } else if let Some(endpoint) = out_config.strip_prefix("statsd=") {
                    println!("Exporting metrics to StatsD: {}", endpoint);
                    let exporter = fusillade::stats::statsd::StatsdExporter::new(endpoint);
                    if let Err(e) = exporter.export(&report) {
                        eprintln!("StatsD export failed: {}", e);
                    } else {
                        println!("StatsD export successful!");
                    }
                }
            }

            // Save to history database if requested
            if save_history {
                match fusillade::stats::db::HistoryDb::open_default() {
                    Ok(db) => {
                        let scenario_name = scenario
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        match db.save_run(&scenario_name, &report) {
                            Ok(id) => println!("Saved to history as run #{}", id),
                            Err(e) => eprintln!("Failed to save history: {}", e),
                        }
                    }
                    Err(e) => eprintln!("Failed to open history database: {}", e),
                }
            }

            // Set capture errors path if flag is provided
            if let Some(path_opt) = capture_errors {
                let path = path_opt
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "fusillade-errors.json".to_string());
                fusillade::bridge::set_capture_errors_path(path);
            }

            Ok(())
        }
        Commands::Init { output, config } => {
            fusillade::cli::init::run_init(output.as_deref(), config)
        }
        Commands::Validate { scenario, config } => {
            fusillade::cli::validate::run_validate(&scenario, config.as_deref())
        }
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            generate(shell, &mut cmd, "fusillade", &mut std::io::stdout());
            Ok(())
        }
        Commands::Exec { script } => {
            let engine = Engine::new().unwrap();
            let _ = engine.run_script(&script);
            Ok(())
        }
        Commands::Worker { listen, connect } => {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let engine = Engine::new().unwrap();
                let server = WorkerServer::new(engine);
                if let Some(addr) = connect {
                    server.connect_to_controller(addr).await.unwrap();
                } else {
                    server.run(listen).await.unwrap();
                }
            });
            Ok(())
        }
        Commands::Controller { listen } => {
            let rt = Runtime::new().unwrap();
            rt.block_on(async {
                let aggregator = std::sync::Arc::new(std::sync::RwLock::new(
                    fusillade::stats::StatsAggregator::new(),
                ));
                let server = ControllerServer::new(aggregator);
                server.run(listen).await
            })
            .unwrap();
            Ok(())
        }
        Commands::Convert { input, output } => {
            let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
            let script = match ext {
                "yaml" | "yml" => fusillade::cli::openapi::convert_to_js(&input)?,
                "json" => {
                    // Try HAR first (has "log" key), otherwise try OpenAPI
                    let content = std::fs::read_to_string(&input)?;
                    if content.contains("\"log\"") {
                        fusillade::cli::har::convert_from_string(&content)?
                    } else {
                        fusillade::cli::openapi::convert_to_js(&input)?
                    }
                }
                _ => fusillade::cli::har::convert_to_js(&input)?,
            };
            if let Some(out_path) = output {
                std::fs::write(out_path, script)?;
            } else {
                println!("{}", script);
            }
            Ok(())
        }
        Commands::Schema { output } => {
            let schema = schemars::schema_for!(fusillade::cli::config::Config);
            let schema_json = serde_json::to_string_pretty(&schema)?;
            if let Some(out_path) = output {
                std::fs::write(&out_path, schema_json)?;
                println!("JSON Schema written to {:?}", out_path);
            } else {
                println!("{}", schema_json);
            }
            Ok(())
        }
        Commands::Record { output, port } => {
            let rt = Runtime::new().unwrap();
            rt.block_on(fusillade::cli::recorder::run_recorder(output, port))
        }
        Commands::Replay { input, parallel } => {
            use fusillade::bridge::replay::load_captured_requests;

            let requests =
                load_captured_requests(input.to_str().unwrap_or("fusillade-errors.json"))
                    .map_err(|e| anyhow::anyhow!("Failed to load errors file: {}", e))?;

            if requests.is_empty() {
                println!("No failed requests found in file.");
                return Ok(());
            }

            println!(
                "Replaying {} failed requests{}...",
                requests.len(),
                if parallel {
                    " in parallel"
                } else {
                    " sequentially"
                }
            );

            if parallel {
                // Parallel execution using threads
                let handles: Vec<_> = requests
                    .into_iter()
                    .enumerate()
                    .map(|(i, req)| {
                        std::thread::spawn(move || {
                            let client = reqwest::blocking::Client::new();
                            let mut builder = match req.method.as_str() {
                                "GET" => client.get(&req.url),
                                "POST" => client.post(&req.url),
                                "PUT" => client.put(&req.url),
                                "DELETE" => client.delete(&req.url),
                                "PATCH" => client.patch(&req.url),
                                _ => client.get(&req.url),
                            };

                            for (key, value) in &req.headers {
                                builder = builder.header(key, value);
                            }

                            if let Some(body) = &req.body {
                                builder = builder.body(body.clone());
                            }

                            let result = builder.send();
                            (i, req.method.clone(), req.url.clone(), result)
                        })
                    })
                    .collect();

                // Collect results
                let mut results: Vec<_> =
                    handles.into_iter().filter_map(|h| h.join().ok()).collect();
                results.sort_by_key(|(i, _, _, _)| *i);

                for (i, method, url, result) in results {
                    println!("\n[{}] {} {}", i + 1, method, url);
                    match result {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().unwrap_or_default();
                            println!("  Status: {}", status);
                            if body.len() < 500 {
                                println!("  Body: {}", body);
                            } else {
                                println!("  Body: {}... (truncated)", &body[..500]);
                            }
                        }
                        Err(e) => {
                            println!("  Error: {}", e);
                        }
                    }
                }
            } else {
                // Sequential execution with delay
                let client = reqwest::blocking::Client::new();

                for (i, req) in requests.iter().enumerate() {
                    println!(
                        "\n[{}/{}] {} {}",
                        i + 1,
                        requests.len(),
                        req.method,
                        req.url
                    );

                    let mut builder = match req.method.as_str() {
                        "GET" => client.get(&req.url),
                        "POST" => client.post(&req.url),
                        "PUT" => client.put(&req.url),
                        "DELETE" => client.delete(&req.url),
                        "PATCH" => client.patch(&req.url),
                        _ => client.get(&req.url),
                    };

                    for (key, value) in &req.headers {
                        builder = builder.header(key, value);
                    }

                    if let Some(body) = &req.body {
                        builder = builder.body(body.clone());
                    }

                    match builder.send() {
                        Ok(response) => {
                            let status = response.status();
                            let body = response.text().unwrap_or_default();
                            println!("  Status: {}", status);
                            if body.len() < 500 {
                                println!("  Body: {}", body);
                            } else {
                                println!("  Body: {}... (truncated)", &body[..500]);
                            }
                        }
                        Err(e) => {
                            println!("  Error: {}", e);
                        }
                    }

                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }

            println!("\nReplay complete.");
            Ok(())
        }
        Commands::Export {
            input,
            format,
            output,
        } => {
            use fusillade::bridge::replay::{export_to_curl, load_captured_requests};

            let requests =
                load_captured_requests(input.to_str().unwrap_or("fusillade-errors.json"))
                    .map_err(|e| anyhow::anyhow!("Failed to load errors file: {}", e))?;

            if requests.is_empty() {
                println!("No failed requests found in file.");
                return Ok(());
            }

            let exported = match format.as_str() {
                "curl" => export_to_curl(&requests),
                _ => {
                    eprintln!("Unsupported format: {}. Supported: curl", format);
                    return Ok(());
                }
            };

            if let Some(out_path) = output {
                std::fs::write(&out_path, &exported)?;
                println!("Exported {} requests to {:?}", requests.len(), out_path);
            } else {
                println!("{}", exported);
            }

            Ok(())
        }
        Commands::Login { token } => {
            println!("Authenticating with Fusillade Cloud...");

            // Validate token format
            if !token.starts_with("fusi_") {
                eprintln!("Error: Invalid token format. Token should start with 'fusi_'");
                return Ok(());
            }

            // Save token locally
            if let Err(e) = fusillade::cli::cloud::save_token(&token, None) {
                eprintln!("Error saving token: {}", e);
                return Ok(());
            }

            println!("✓ Logged in successfully!");
            println!("  Token stored in ~/.fusillade/auth.json");
            println!("\nYou can now use 'fusillade run --cloud' to run tests on Fusillade Cloud.");
            Ok(())
        }
        Commands::Whoami => {
            match fusillade::cli::cloud::load_token() {
                Some(auth) => {
                    println!("Logged in to Fusillade Cloud");
                    println!("  Token: {}...", &auth.token[..12]);
                    println!("  API:   {}", fusillade::cli::cloud::get_api_url());
                }
                None => {
                    println!("Not logged in.");
                    println!("Run 'fusillade login <token>' to authenticate.");
                    println!("Get your API key at https://fusillade.io/settings");
                }
            }
            Ok(())
        }
        Commands::Logout => {
            if fusillade::cli::cloud::is_logged_in() {
                match fusillade::cli::cloud::clear_token() {
                    Ok(()) => {
                        println!("Logged out successfully.");
                        println!("Token removed from ~/.fusillade/auth.json");
                    }
                    Err(e) => {
                        eprintln!("Error removing token: {}", e);
                    }
                }
            } else {
                println!("Not logged in.");
            }
            Ok(())
        }
        Commands::Status => {
            println!("Fusillade v{}", env!("CARGO_PKG_VERSION"));
            println!();

            // Cloud status
            if fusillade::cli::cloud::is_logged_in() {
                println!("Cloud:    Connected");
                println!("  API:    {}", fusillade::cli::cloud::get_api_url());
            } else {
                println!("Cloud:    Not connected");
            }

            // System info
            let mem = fusillade::engine::memory::MemoryInfo::current();
            println!();
            println!("System:");
            println!(
                "  RAM:    {} / {}",
                fusillade::engine::memory::format_bytes(mem.available_bytes),
                fusillade::engine::memory::format_bytes(mem.total_bytes)
            );
            println!("  CPUs:   {}", num_cpus::get());

            // Estimate max workers
            let max_workers = fusillade::engine::memory::estimate_max_workers(mem.available_bytes);
            println!("  Max Workers: ~{}", max_workers);

            Ok(())
        }
        Commands::History { show, limit } => {
            let db = match fusillade::stats::db::HistoryDb::open_default() {
                Ok(db) => db,
                Err(e) => {
                    eprintln!("Failed to open history database: {}", e);
                    eprintln!("History is stored in fusillade_history.db");
                    return Ok(());
                }
            };

            if let Some(run_id) = show {
                // Show detailed report for specific run
                match db.get_report(run_id) {
                    Ok(report) => {
                        println!("Run #{} Report", run_id);
                        println!("{}", "=".repeat(60));
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report).unwrap_or_default()
                        );
                    }
                    Err(e) => {
                        eprintln!("Failed to load run #{}: {}", run_id, e);
                    }
                }
            } else {
                // List recent runs
                match db.list_runs(limit) {
                    Ok(runs) => {
                        if runs.is_empty() {
                            println!("No test runs in history.");
                            println!("Run a test with --save-history to start tracking.");
                        } else {
                            println!("Recent Test Runs");
                            println!("{}", "=".repeat(60));
                            println!("{:<6} {:<24} Scenario", "ID", "Time");
                            println!("{}", "-".repeat(60));
                            for (id, time, scenario) in runs {
                                println!("{:<6} {:<24} {}", id, time, scenario);
                            }
                            println!();
                            println!("Use 'fusillade history --show <ID>' to view details.");
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to list runs: {}", e);
                    }
                }
            }
            Ok(())
        }
        Commands::Compare { baseline, current } => {
            // Load both JSON summaries
            let baseline_content = std::fs::read_to_string(&baseline)
                .map_err(|e| anyhow::anyhow!("Failed to read baseline: {}", e))?;
            let current_content = std::fs::read_to_string(&current)
                .map_err(|e| anyhow::anyhow!("Failed to read current: {}", e))?;

            let baseline_json: serde_json::Value = serde_json::from_str(&baseline_content)
                .map_err(|e| anyhow::anyhow!("Invalid baseline JSON: {}", e))?;
            let current_json: serde_json::Value = serde_json::from_str(&current_content)
                .map_err(|e| anyhow::anyhow!("Invalid current JSON: {}", e))?;

            println!(
                "Comparison: {} vs {}",
                baseline.display(),
                current.display()
            );
            println!("{}", "=".repeat(60));

            // Compare key metrics
            fn get_f64(v: &serde_json::Value, key: &str) -> Option<f64> {
                v.get(key).and_then(|x| x.as_f64())
            }
            fn get_u64(v: &serde_json::Value, key: &str) -> Option<u64> {
                v.get(key).and_then(|x| x.as_u64())
            }

            fn format_change(baseline: f64, current: f64, lower_is_better: bool) -> String {
                let diff = current - baseline;
                let pct = if baseline > 0.0 {
                    (diff / baseline) * 100.0
                } else {
                    0.0
                };
                let arrow = if diff > 0.0 { "+" } else { "" };
                let color = if (lower_is_better && diff < 0.0) || (!lower_is_better && diff > 0.0) {
                    "\x1b[32m" // Green - improvement
                } else if diff.abs() < 0.01 {
                    "\x1b[0m" // No change
                } else {
                    "\x1b[31m" // Red - regression
                };
                format!("{}{}{:.1}%\x1b[0m", color, arrow, pct)
            }

            // Total requests
            if let (Some(b), Some(c)) = (
                get_u64(&baseline_json, "total_requests"),
                get_u64(&current_json, "total_requests"),
            ) {
                let pct = format_change(b as f64, c as f64, false);
                println!("Total Requests:     {:>10} -> {:>10}  ({})", b, c, pct);
            }

            // Average latency
            if let (Some(b), Some(c)) = (
                get_f64(&baseline_json, "avg_latency_ms"),
                get_f64(&current_json, "avg_latency_ms"),
            ) {
                let pct = format_change(b, c, true);
                println!("Avg Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // P95 latency
            if let (Some(b), Some(c)) = (
                get_f64(&baseline_json, "p95_latency_ms"),
                get_f64(&current_json, "p95_latency_ms"),
            ) {
                let pct = format_change(b, c, true);
                println!("P95 Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // P99 latency
            if let (Some(b), Some(c)) = (
                get_f64(&baseline_json, "p99_latency_ms"),
                get_f64(&current_json, "p99_latency_ms"),
            ) {
                let pct = format_change(b, c, true);
                println!("P99 Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // Max latency
            if let (Some(b), Some(c)) = (
                get_f64(&baseline_json, "max_latency_ms"),
                get_f64(&current_json, "max_latency_ms"),
            ) {
                let pct = format_change(b, c, true);
                println!("Max Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // RPS
            if let (Some(b), Some(c)) = (
                get_f64(&baseline_json, "rps"),
                get_f64(&current_json, "rps"),
            ) {
                let pct = format_change(b, c, false);
                println!("RPS:                {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // Error rate (if errors key exists)
            let baseline_errors = baseline_json
                .get("errors")
                .and_then(|e| e.as_object())
                .map(|e| e.values().filter_map(|v| v.as_u64()).sum::<u64>())
                .unwrap_or(0);
            let current_errors = current_json
                .get("errors")
                .and_then(|e| e.as_object())
                .map(|e| e.values().filter_map(|v| v.as_u64()).sum::<u64>())
                .unwrap_or(0);
            let baseline_total = get_u64(&baseline_json, "total_requests").unwrap_or(1);
            let current_total = get_u64(&current_json, "total_requests").unwrap_or(1);
            let baseline_error_rate = (baseline_errors as f64 / baseline_total as f64) * 100.0;
            let current_error_rate = (current_errors as f64 / current_total as f64) * 100.0;
            let pct = format_change(baseline_error_rate, current_error_rate, true);
            println!(
                "Error Rate (%):     {:>10.2} -> {:>10.2}  ({})",
                baseline_error_rate, current_error_rate, pct
            );

            println!("{}", "=".repeat(60));
            println!("\x1b[32mGreen\x1b[0m = improvement, \x1b[31mRed\x1b[0m = regression");

            Ok(())
        }
    }
}
