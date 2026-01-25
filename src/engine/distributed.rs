use crate::cli::config::Config;
use crate::cluster::proto::cluster_client::ClusterClient;
use crate::cluster::proto::{
    controller_command, worker_message, RegisterRequest, TestFinished, WorkerMessage,
};
use crate::engine::Engine;
use crate::stats::db::HistoryDb;
use crate::stats::{Metric, ReportStats, SharedAggregator};
use anyhow::Result;
use axum::{
    extract::State,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Deserialize, Serialize)]
pub struct StartTestRequest {
    pub script_content: String,
    pub script_name: String,
    pub config: Config,
    pub controller_metrics_url: Option<String>,
    pub assets: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Serialize)]
pub struct MetricBatch {
    pub worker_id: usize,
    pub metrics: Vec<Metric>,
}

#[derive(Serialize)]
pub struct WorkerStatus {
    pub status: String,
}

pub struct WorkerServer {
    engine: Arc<Engine>,
}

impl WorkerServer {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    pub async fn run(self, addr: String) -> Result<()> {
        let app = Router::new()
            .route("/status", get(handle_status))
            .route("/start", post(handle_start))
            .with_state(self.engine);

        println!("Worker listening on {}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }

    pub async fn connect_to_controller(&self, controller_addr: String) -> Result<()> {
        let grpc_addr = if controller_addr.starts_with("http") {
            controller_addr
        } else {
            format!("http://{}", controller_addr)
        };

        println!("Connecting to Controller at {}...", grpc_addr);
        let mut client = ClusterClient::connect(grpc_addr).await?;
        println!("Connected!");

        let (tx, rx) = tokio::sync::mpsc::channel(10);
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        let request = tonic::Request::new(stream);
        let response = client.register(request).await?;
        let mut inbound = response.into_inner();

        let worker_id = uuid::Uuid::new_v4().to_string();
        let reg = WorkerMessage {
            msg: Some(worker_message::Msg::Register(RegisterRequest {
                id: worker_id.clone(),
                address: "unknown".to_string(),
                available_cpus: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1) as u32,
            })),
        };
        tx.send(reg).await?;

        println!("Registered as {}. Waiting for commands...", worker_id);

        let engine = self.engine.clone();
        let tx_clone = tx.clone();
        let worker_id_clone = worker_id.clone();

        while let Some(cmd) = inbound.message().await? {
            match cmd.cmd {
                Some(controller_command::Cmd::StartTest(start_test)) => {
                    println!("Received StartTest command");

                    // Parse config from JSON
                    let config: Config =
                        serde_json::from_str(&start_test.config_json).unwrap_or_default();

                    // Save assets to local disk before running
                    for (path, content) in &start_test.assets {
                        let p = PathBuf::from(path);
                        if let Some(parent) = p.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(p, content);
                    }

                    let script_content = start_test.script_content.clone();
                    let script_path = PathBuf::from("distributed_test.js");
                    let engine_clone = engine.clone();
                    let tx_finished = tx_clone.clone();
                    let wid = worker_id_clone.clone();

                    // Run test in background
                    tokio::spawn(async move {
                        println!("Starting load test...");
                        let result = engine_clone.run_load_test(
                            script_path,
                            script_content,
                            config,
                            true, // json output
                            None, // export_json
                            None, // export_html
                            None, // metrics_url (TODO: stream to controller)
                            None, // metrics_auth
                            None, // control_rx
                        );

                        match result {
                            Ok(report) => {
                                println!(
                                    "Test completed: {} requests, avg {}ms",
                                    report.total_requests, report.avg_latency_ms
                                );
                            }
                            Err(e) => {
                                eprintln!("Test failed: {}", e);
                            }
                        }

                        // Notify controller that test is finished
                        let finished = WorkerMessage {
                            msg: Some(worker_message::Msg::TestFinished(TestFinished {
                                worker_id: wid,
                            })),
                        };
                        let _ = tx_finished.send(finished).await;
                    });
                }
                Some(controller_command::Cmd::StopTest(_)) => {
                    println!("Received StopTest command");
                    // TODO: Implement graceful stop
                }
                None => {
                    println!("Received empty command");
                }
            }
        }

        Ok(())
    }
}

async fn handle_status() -> Json<WorkerStatus> {
    Json(WorkerStatus {
        status: "ready".to_string(),
    })
}

async fn handle_start(
    State(engine): State<Arc<Engine>>,
    Json(payload): Json<StartTestRequest>,
) -> Json<WorkerStatus> {
    let script_path = PathBuf::from(payload.script_name);
    let script_content = payload.script_content;
    let config = payload.config;
    let metrics_url = payload.controller_metrics_url;

    // Save assets to local disk before running
    if let Some(assets) = payload.assets {
        for (path, content) in assets {
            let p = PathBuf::from(&path);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(p, content);
        }
    }

    // Spawn the test in a background task
    tokio::spawn(async move {
        // Run load test. If metrics_url is present, we need to stream metrics there.
        let _ = engine.run_load_test(
            script_path,
            script_content,
            config,
            true,
            None,
            None,
            metrics_url,
            None,
            None,
        );
    });

    Json(WorkerStatus {
        status: "started".to_string(),
    })
}

// --- Controller Side Logic ---

use crate::cluster::{dispatch_test_to_workers, WorkerRegistry};

/// Shared state for the controller HTTP API
#[derive(Clone)]
pub struct ControllerState {
    pub aggregator: SharedAggregator,
    pub workers: WorkerRegistry,
}

pub struct ControllerServer {
    aggregator: SharedAggregator,
}

impl ControllerServer {
    pub fn new(aggregator: SharedAggregator) -> Self {
        Self { aggregator }
    }

    pub async fn run(self, addr: String) -> Result<()> {
        // Create shared worker registry
        let workers: WorkerRegistry =
            std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));

        // Start gRPC server on port + 1
        let grpc_addr = addr.parse::<std::net::SocketAddr>()?;
        let grpc_port = grpc_addr.port() + 1;
        let grpc_addr = std::net::SocketAddr::new(grpc_addr.ip(), grpc_port);

        println!("Controller gRPC listening on {}", grpc_addr);

        let cluster_service = crate::cluster::ClusterService::new_with_registry(
            self.aggregator.clone(),
            workers.clone(),
        );
        let svc = crate::cluster::proto::cluster_server::ClusterServer::new(cluster_service);

        tokio::spawn(async move {
            if let Err(e) = tonic::transport::Server::builder()
                .add_service(svc)
                .serve(grpc_addr)
                .await
            {
                eprintln!("gRPC Server Error: {}", e);
            }
        });

        let state = ControllerState {
            aggregator: self.aggregator,
            workers,
        };

        let app = Router::new()
            .route("/", get(handle_index))
            .route("/dashboard", get(handle_dashboard))
            .route("/metrics", post(handle_metrics))
            .route("/api/stats", get(handle_api_stats))
            .route("/api/history", get(handle_api_history))
            .route("/api/screenshots", get(handle_api_screenshots))
            .route("/api/workers", get(handle_api_workers))
            .route("/api/dispatch", post(handle_api_dispatch))
            .nest_service("/screenshots", ServeDir::new("screenshots"))
            .with_state(state);

        println!("Controller running on http://{}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn handle_index() -> Html<String> {
    Html(include_str!("../../src/tui/index.html").to_string())
}

async fn handle_dashboard() -> Html<String> {
    Html(include_str!("../../src/tui/dashboard.html").to_string())
}

async fn handle_metrics(State(state): State<ControllerState>, Json(batch): Json<MetricBatch>) {
    if let Ok(mut guard) = state.aggregator.write() {
        for metric in batch.metrics {
            guard.add(metric);
        }
    }
}

async fn handle_api_stats(State(state): State<ControllerState>) -> Json<ReportStats> {
    let guard = state.aggregator.read().unwrap();
    Json(guard.to_report())
}

async fn handle_api_history() -> Json<Vec<(i64, String, String)>> {
    if let Ok(db) = HistoryDb::open_default() {
        if let Ok(runs) = db.list_runs(50) {
            return Json(runs);
        }
    }
    Json(vec![])
}

async fn handle_api_screenshots() -> Json<Vec<String>> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir("screenshots") {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                files.push(name);
            }
        }
    }
    files.sort_by(|a, b| b.cmp(a));
    Json(files)
}

/// Response for /api/workers endpoint
#[derive(Serialize)]
pub struct WorkerListResponse {
    pub workers: Vec<WorkerInfoResponse>,
}

#[derive(Serialize)]
pub struct WorkerInfoResponse {
    pub id: String,
    pub address: String,
    pub available_cpus: u32,
}

async fn handle_api_workers(State(state): State<ControllerState>) -> Json<WorkerListResponse> {
    let workers = match state.workers.read() {
        Ok(guard) => guard
            .values()
            .map(|w| WorkerInfoResponse {
                id: w.id.clone(),
                address: w.address.clone(),
                available_cpus: w.available_cpus,
            })
            .collect(),
        Err(_) => vec![],
    };
    Json(WorkerListResponse { workers })
}

/// Request body for /api/dispatch endpoint
#[derive(Deserialize)]
pub struct DispatchTestRequest {
    pub script_content: String,
    pub config: Config,
    #[serde(default)]
    pub assets: std::collections::HashMap<String, String>,
}

/// Response for /api/dispatch endpoint
#[derive(Serialize)]
pub struct DispatchTestResponse {
    pub success: bool,
    pub workers_dispatched: usize,
    pub message: String,
}

async fn handle_api_dispatch(
    State(state): State<ControllerState>,
    Json(payload): Json<DispatchTestRequest>,
) -> Json<DispatchTestResponse> {
    let config_json = match serde_json::to_string(&payload.config) {
        Ok(json) => json,
        Err(e) => {
            return Json(DispatchTestResponse {
                success: false,
                workers_dispatched: 0,
                message: format!("Failed to serialize config: {}", e),
            });
        }
    };

    match dispatch_test_to_workers(
        &state.workers,
        payload.script_content,
        config_json,
        payload.assets,
    )
    .await
    {
        Ok(count) => Json(DispatchTestResponse {
            success: true,
            workers_dispatched: count,
            message: format!("Test dispatched to {} workers", count),
        }),
        Err(e) => Json(DispatchTestResponse {
            success: false,
            workers_dispatched: 0,
            message: e,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_test_request_serialization() {
        let config = Config::default();
        let req = StartTestRequest {
            script_content: "export default function() {}".to_string(),
            script_name: "test.js".to_string(),
            config,
            controller_metrics_url: Some("http://localhost:9000".to_string()),
            assets: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: StartTestRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.script_name, "test.js");
        assert_eq!(
            deserialized.controller_metrics_url,
            Some("http://localhost:9000".to_string())
        );
    }

    #[test]
    fn test_worker_status_serialization() {
        let status = WorkerStatus {
            status: "ready".to_string(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(json, r#"{"status":"ready"}"#);
    }

    #[test]
    fn test_dispatch_response_serialization() {
        let resp = DispatchTestResponse {
            success: true,
            workers_dispatched: 5,
            message: "Go".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("true"));
        assert!(json.contains("5"));
    }
}
