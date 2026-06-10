//! End-to-end engine tests against a local HTTP server.
//!
//! These are the only tests that exercise the full pipeline: JS script ->
//! engine workers -> real sockets -> metrics aggregation -> final report.
//! The server lives on an ephemeral 127.0.0.1 port, so the suite is fully
//! hermetic (no httpbin.org).

use fusillade::cli::config::Config;
use fusillade::stats::ReportStats;
use fusillade::Engine;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Minimal HTTP/1.1 server with keep-alive, one thread per connection.
struct TestServer {
    /// Base URL, e.g. "http://127.0.0.1:34567"
    base_url: String,
    /// Total requests served across all connections.
    hits: Arc<AtomicUsize>,
}

impl TestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_for_accept = hits.clone();

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let hits = hits_for_accept.clone();
                std::thread::spawn(move || handle_connection(stream, hits));
            }
        });

        TestServer { base_url, hits }
    }
}

fn handle_connection(mut stream: TcpStream, hits: Arc<AtomicUsize>) {
    loop {
        // Read request head byte-by-byte until the blank line.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) => return,
                Ok(_) => {
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => return,
            }
        }

        let head = String::from_utf8_lossy(&head).to_string();
        let request_line = head.lines().next().unwrap_or_default().to_string();
        let content_length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);

        let mut body = vec![0u8; content_length];
        if content_length > 0 && stream.read_exact(&mut body).is_err() {
            return;
        }

        hits.fetch_add(1, Ordering::SeqCst);

        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("/");

        let (status_line, response_body): (&str, Vec<u8>) = match (method, path) {
            ("GET", "/ok") => ("200 OK", b"hello world".to_vec()),
            ("GET", "/status/500") => ("500 Internal Server Error", b"oops".to_vec()),
            ("POST", "/echo") => ("200 OK", body.clone()),
            ("GET", "/slow") => {
                std::thread::sleep(std::time::Duration::from_secs(3));
                ("200 OK", b"finally".to_vec())
            }
            _ => ("404 Not Found", b"not found".to_vec()),
        };

        let header = format!(
            "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: keep-alive\r\n\r\n",
            status_line,
            response_body.len()
        );
        if stream.write_all(header.as_bytes()).is_err() || stream.write_all(&response_body).is_err()
        {
            return;
        }
    }
}

/// Engine runs spawn worker threads and a may green-thread pool; serialize
/// them so parallel #[test] threads don't share schedulers mid-run.
static ENGINE_LOCK: Mutex<()> = Mutex::new(());

fn run_script(script: String, config: Config) -> ReportStats {
    let _guard = ENGINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let engine = Arc::new(Engine::new().expect("engine"));
    engine
        .run_load_test(
            PathBuf::from("integration_test.js"),
            script,
            config,
            true, // json_output: keep test output machine-shaped
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("load test run")
}

fn iterations_config(workers: usize, iterations: u64) -> Config {
    Config {
        workers: Some(workers),
        iterations: Some(iterations),
        ..Default::default()
    }
}

#[test]
fn full_run_reports_exact_request_count_statuses_and_checks() {
    let server = TestServer::start();
    let script = format!(
        r#"
export default function () {{
    let res = http.get('{base}/ok');
    check(res, {{
        'status is 200': (r) => r.status === 200,
        'body is hello': (r) => r.body === 'hello world',
    }});
}}
"#,
        base = server.base_url
    );

    // 2 workers x 3 iterations x 1 request = exactly 6 requests
    let report = run_script(script, iterations_config(2, 3));

    assert_eq!(report.total_requests, 6);
    assert_eq!(report.status_codes.get(&200), Some(&6));
    assert!(report.errors.is_empty(), "errors: {:?}", report.errors);
    assert_eq!(report.checks.get("status is 200"), Some(&(6, 6)));
    assert_eq!(report.checks.get("body is hello"), Some(&(6, 6)));
    assert_eq!(server.hits.load(Ordering::SeqCst), 6);
    assert!(report.avg_latency_ms > 0.0);
    assert!(report.total_data_received > 0);
}

#[test]
fn failing_checks_are_counted_as_failures_not_dropped() {
    let server = TestServer::start();
    let script = format!(
        r#"
export default function () {{
    let res = http.get('{base}/ok');
    check(res, {{
        'always passes': (r) => r.status === 200,
        'always fails': () => false,
    }});
}}
"#,
        base = server.base_url
    );

    let report = run_script(script, iterations_config(1, 4));

    // Checks are stored as (total, passes)
    assert_eq!(report.checks.get("always passes"), Some(&(4, 4)));
    assert_eq!(report.checks.get("always fails"), Some(&(4, 0)));
}

#[test]
fn post_body_round_trips_through_engine_and_server() {
    let server = TestServer::start();
    let script = format!(
        r#"
export default function () {{
    let res = http.post('{base}/echo', 'ping-pong-payload', {{
        headers: {{ 'Content-Type': 'text/plain' }},
    }});
    check(res, {{
        'echo status 200': (r) => r.status === 200,
        'echo body matches': (r) => r.body === 'ping-pong-payload',
    }});
}}
"#,
        base = server.base_url
    );

    let report = run_script(script, iterations_config(1, 2));

    assert_eq!(report.checks.get("echo status 200"), Some(&(2, 2)));
    assert_eq!(report.checks.get("echo body matches"), Some(&(2, 2)));
    assert!(report.total_data_sent > 0);
}

#[test]
fn non_2xx_statuses_are_tracked_per_code() {
    let server = TestServer::start();
    let script = format!(
        r#"
export default function () {{
    http.get('{base}/status/500');
}}
"#,
        base = server.base_url
    );

    let report = run_script(script, iterations_config(1, 2));

    assert_eq!(report.total_requests, 2);
    assert_eq!(report.status_codes.get(&500), Some(&2));
}

#[test]
fn request_timeout_yields_status_zero() {
    let server = TestServer::start();
    let script = format!(
        r#"
export default function () {{
    let res = http.get('{base}/slow', {{ timeout: '300ms' }});
    check(res, {{
        'timed out with status 0': (r) => r.status === 0,
    }});
}}
"#,
        base = server.base_url
    );

    let report = run_script(script, iterations_config(1, 1));

    assert_eq!(report.checks.get("timed out with status 0"), Some(&(1, 1)));
}
