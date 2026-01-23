use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::Runtime;



use fusillade::engine::Engine;
use fusillade::engine::distributed::{WorkerServer, ControllerServer};
use std::time::Duration;

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
    
    let script_name = scenario.file_name()
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
        .text("duration", parse_duration_str(duration.as_deref().unwrap_or("60s"))
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|| "60".to_string()))
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
            let test_id = body.get("test_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            
            println!("\n✓ Test submitted successfully!");
            println!("  Test ID: {}", test_id);
            println!("  Status: pending");
            println!("\nView results at: https://fusillade.io/dashboard/runs/{}", test_id);
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
enum Commands {
    Run {
        scenario: PathBuf,
        #[arg(short, long, alias = "vus")]
        workers: Option<usize>,
        #[arg(short, long)]
        duration: Option<String>,
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
        #[arg(long)]
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
    Types {
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
    /// Check authentication status
    Whoami,
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
        Commands::Run { scenario, workers, duration, headless, json, export_json, export_html, out, metrics_url, metrics_auth, jitter, drop, estimate_cost, interactive, cloud, region, no_endpoint_tracking, watch, no_memory_check, memory_safe } => {
            // Load .env file if present (check script directory, then current directory)
            let script_dir = scenario.parent().unwrap_or(std::path::Path::new("."));
            let env_paths = [
                script_dir.join(".env"),
                PathBuf::from(".env"),
            ];
            for env_path in &env_paths {
                if env_path.exists() {
                    if let Ok(contents) = std::fs::read_to_string(env_path) {
                        for line in contents.lines() {
                            let line = line.trim();
                            if line.is_empty() || line.starts_with('#') { continue; }
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

                let mut last_modified = std::fs::metadata(&scenario)
                    .and_then(|m| m.modified())
                    .ok();

                loop {
                    // Run test with reduced settings for dev workflow
                    let engine = Engine::new()?;
                    let script_content = std::fs::read_to_string(&scenario).unwrap();
                    let mut watch_config = engine.extract_config(scenario.clone(), script_content.clone()).unwrap().unwrap_or_default();

                    // Use provided values or defaults for watch mode
                    if let Some(w) = workers { watch_config.workers = Some(w); }
                    else if watch_config.workers.is_none() { watch_config.workers = Some(1); }

                    if let Some(d) = duration.clone() { watch_config.duration = Some(d); }
                    else if watch_config.duration.is_none() { watch_config.duration = Some("5s".to_string()); }

                    if let Some(j) = jitter.clone() { watch_config.jitter = Some(j); }
                    if let Some(p) = drop { watch_config.drop = Some(p); }
                    if no_endpoint_tracking { watch_config.no_endpoint_tracking = Some(true); }

                    println!("--- Running test (workers: {}, duration: {}) ---",
                        watch_config.workers.unwrap_or(1),
                        watch_config.duration.as_deref().unwrap_or("5s"));

                    let engine_arc = Arc::new(engine);
                    let _ = engine_arc.run_load_test(
                        scenario.clone(),
                        script_content,
                        watch_config,
                        true, // Always headless in watch mode
                        None, None, None, None, None
                    );

                    println!("\n--- Waiting for changes to {} ---\n", scenario.display());

                    // Poll for file changes
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        let current_modified = std::fs::metadata(&scenario)
                            .and_then(|m| m.modified())
                            .ok();

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
            let mut final_config = engine.extract_config(scenario.clone(), script_content.clone()).unwrap().unwrap_or_default();

            if let Some(w) = workers { final_config.workers = Some(w); }
            if let Some(d) = duration { final_config.duration = Some(d); }
            if let Some(j) = jitter { final_config.jitter = Some(j); }
            if let Some(p) = drop { final_config.drop = Some(p); }
            if no_endpoint_tracking { final_config.no_endpoint_tracking = Some(true); }

            // Pre-flight memory check (unless disabled)
            if !no_memory_check {
                let requested_workers = final_config.workers.unwrap_or(1);
                let preflight = fusillade::engine::memory::preflight_check(requested_workers);
                
                if !preflight.safe {
                    eprintln!("╭────────────────────────────────────────────────────────────╮");
                    eprintln!("│ ⚠️  MEMORY WARNING                                          │");
                    eprintln!("├────────────────────────────────────────────────────────────┤");
                    eprintln!("│ Requested workers: {:>8}                                │", preflight.requested);
                    eprintln!("│ Estimated max:     {:>8} (based on available RAM)       │", preflight.estimated_max);
                    eprintln!("│ Available RAM:     {:>8}                                │", fusillade::engine::memory::format_bytes(preflight.available_bytes));
                    eprintln!("│ Estimated needed:  {:>8}                                │", fusillade::engine::memory::format_bytes(preflight.estimated_needed));
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
            if !headless_mode { println!("Running scenario..."); }
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
                let dry_report = engine_arc.clone().run_load_test(scenario.clone(), script_content.clone(), dry_config, true, None, None, None, None, None)?;
                
                // Calculate estimates
                let requests = dry_report.total_requests.max(1) as f64;
                let avg_response_bytes: f64 = dry_report.grouped_requests.values()
                    .map(|r| r.avg_receiving_ms * 1024.0) // Approximate: use receive time as proxy when size unavailable
                    .sum::<f64>() / requests;
                
                // Estimate based on requested config
                let target_workers = final_config.workers.unwrap_or(1) as f64;
                let target_duration_secs = parse_duration_str(final_config.duration.as_deref().unwrap_or("60s")).map(|d| d.as_secs_f64()).unwrap_or(60.0);
                let avg_req_duration_secs = dry_report.avg_latency_ms / 1000.0;
                let est_total_requests = if avg_req_duration_secs > 0.0 {
                    (target_workers * target_duration_secs / avg_req_duration_secs) as u64
                } else {
                    (target_workers * target_duration_secs * 10.0) as u64 // Fallback: 10 req/s per worker
                };
                
                // Assume ~10KB average response if we can't measure (conservative estimate)
                let est_bytes_per_req = if avg_response_bytes > 0.0 { avg_response_bytes } else { 10_000.0 };
                let est_total_bytes = est_total_requests as f64 * est_bytes_per_req * 2.0; // *2 for request+response
                let est_gb = est_total_bytes / 1_073_741_824.0;
                let est_cost = est_gb * 0.09; // AWS standard egress cost
                
                // Only show warning if estimated cost exceeds user-defined threshold
                if est_cost > cost_threshold {
                    println!("\nWARNING: Estimated cost exceeds ${:.0} threshold", cost_threshold);
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

            // Setup interactive control if requested
            let control_rx = if interactive {
                use fusillade::engine::control::{ControlCommand, parse_control_command};
                use std::io::BufRead;
                
                let (tx, rx) = std::sync::mpsc::channel::<ControlCommand>();
                
                println!("Interactive mode enabled. Commands: ramp <N>, pause, resume, status, stop");
                println!("   Type commands and press Enter.\n");
                
                // Spawn input thread
                std::thread::spawn(move || {
                    let stdin = std::io::stdin();
                    for line in stdin.lock().lines().map_while(Result::ok) {
                        if let Some(cmd) = parse_control_command(&line) {
                            if tx.send(cmd).is_err() {
                                break; // Channel closed
                            }
                        } else if !line.trim().is_empty() {
                            println!("Unknown command: {}", line.trim());
                        }
                    }
                });
                
                Some(rx)
            } else {
                None
            };
            
            let report = engine_arc.run_load_test(scenario.clone(), script_content, final_config, headless_mode, export_json, export_html, metrics_url, metrics_auth, control_rx)?;

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
                }
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
                let aggregator = std::sync::Arc::new(std::sync::RwLock::new(fusillade::stats::StatsAggregator::new()));
                let server = ControllerServer::new(aggregator);
                server.run(listen).await
            }).unwrap();
            Ok(())
        }
        Commands::Convert { input, output } => {
            let script = fusillade::cli::har::convert_to_js(&input)?;
            if let Some(out_path) = output {
                std::fs::write(out_path, script)?;
            } else {
                println!("{}", script);
            }
            Ok(())
        }
        Commands::Types { output } => {
            let types = fusillade::cli::types::generate_d_ts();
            if let Some(out_path) = output {
                std::fs::write(&out_path, types)?;
                println!("Type definitions written to {:?}", out_path);
            } else {
                println!("{}", types);
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
            
            let requests = load_captured_requests(input.to_str().unwrap_or("fusillade-errors.json"))
                .map_err(|e| anyhow::anyhow!("Failed to load errors file: {}", e))?;
            
            if requests.is_empty() {
                println!("No failed requests found in file.");
                return Ok(());
            }
            
            println!("Replaying {} failed requests...", requests.len());
            
            let client = reqwest::blocking::Client::new();
            
            for (i, req) in requests.iter().enumerate() {
                println!("\n[{}/{}] {} {}", i + 1, requests.len(), req.method, req.url);
                
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
                
                if !parallel {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            
            println!("\nReplay complete.");
            Ok(())
        }
        Commands::Export { input, format, output } => {
            use fusillade::bridge::replay::{load_captured_requests, export_to_curl};
            
            let requests = load_captured_requests(input.to_str().unwrap_or("fusillade-errors.json"))
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

            println!("Comparison: {} vs {}", baseline.display(), current.display());
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
                let pct = if baseline > 0.0 { (diff / baseline) * 100.0 } else { 0.0 };
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
            if let (Some(b), Some(c)) = (get_u64(&baseline_json, "total_requests"), get_u64(&current_json, "total_requests")) {
                let pct = format_change(b as f64, c as f64, false);
                println!("Total Requests:     {:>10} -> {:>10}  ({})", b, c, pct);
            }

            // Average latency
            if let (Some(b), Some(c)) = (get_f64(&baseline_json, "avg_latency_ms"), get_f64(&current_json, "avg_latency_ms")) {
                let pct = format_change(b, c, true);
                println!("Avg Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // P95 latency
            if let (Some(b), Some(c)) = (get_f64(&baseline_json, "p95_latency_ms"), get_f64(&current_json, "p95_latency_ms")) {
                let pct = format_change(b, c, true);
                println!("P95 Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // P99 latency
            if let (Some(b), Some(c)) = (get_f64(&baseline_json, "p99_latency_ms"), get_f64(&current_json, "p99_latency_ms")) {
                let pct = format_change(b, c, true);
                println!("P99 Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // Max latency
            if let (Some(b), Some(c)) = (get_f64(&baseline_json, "max_latency_ms"), get_f64(&current_json, "max_latency_ms")) {
                let pct = format_change(b, c, true);
                println!("Max Latency (ms):   {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // RPS
            if let (Some(b), Some(c)) = (get_f64(&baseline_json, "rps"), get_f64(&current_json, "rps")) {
                let pct = format_change(b, c, false);
                println!("RPS:                {:>10.2} -> {:>10.2}  ({})", b, c, pct);
            }

            // Error rate (if errors key exists)
            let baseline_errors = baseline_json.get("errors").and_then(|e| e.as_object()).map(|e| e.values().filter_map(|v| v.as_u64()).sum::<u64>()).unwrap_or(0);
            let current_errors = current_json.get("errors").and_then(|e| e.as_object()).map(|e| e.values().filter_map(|v| v.as_u64()).sum::<u64>()).unwrap_or(0);
            let baseline_total = get_u64(&baseline_json, "total_requests").unwrap_or(1);
            let current_total = get_u64(&current_json, "total_requests").unwrap_or(1);
            let baseline_error_rate = (baseline_errors as f64 / baseline_total as f64) * 100.0;
            let current_error_rate = (current_errors as f64 / current_total as f64) * 100.0;
            let pct = format_change(baseline_error_rate, current_error_rate, true);
            println!("Error Rate (%):     {:>10.2} -> {:>10.2}  ({})", baseline_error_rate, current_error_rate, pct);

            println!("{}", "=".repeat(60));
            println!("\x1b[32mGreen\x1b[0m = improvement, \x1b[31mRed\x1b[0m = regression");

            Ok(())
        }
    }
}