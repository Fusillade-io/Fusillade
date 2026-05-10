pub mod proto {
    tonic::include_proto!("cluster");
}

use crate::stats::{Metric as InternalMetric, SharedAggregator};
use futures::Stream;
use proto::cluster_server::Cluster;
use proto::{
    controller_command, worker_message, ControllerCommand, Empty, MetricBatch, StartTest,
    WorkerMessage,
};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tonic::{Request, Response, Status, Streaming};

/// Information about a connected worker
#[derive(Clone)]
pub struct WorkerInfo {
    pub id: String,
    pub address: String,
    pub available_cpus: u32,
    pub sender: mpsc::Sender<Result<ControllerCommand, Status>>,
}

/// Shared state for tracking workers
pub type WorkerRegistry = Arc<RwLock<HashMap<String, WorkerInfo>>>;

pub struct ClusterService {
    aggregator: SharedAggregator,
    workers: WorkerRegistry,
    cluster_token: Option<String>,
}

impl ClusterService {
    pub fn new(aggregator: SharedAggregator) -> Self {
        Self {
            aggregator,
            workers: Arc::new(RwLock::new(HashMap::new())),
            cluster_token: None,
        }
    }

    pub fn new_with_registry(
        aggregator: SharedAggregator,
        workers: WorkerRegistry,
        cluster_token: Option<String>,
    ) -> Self {
        Self {
            aggregator,
            workers,
            cluster_token,
        }
    }

    pub fn workers(&self) -> WorkerRegistry {
        self.workers.clone()
    }

    #[allow(clippy::result_large_err)]
    fn check_auth(&self, request_meta: &tonic::metadata::MetadataMap) -> Result<(), Status> {
        let expected = match &self.cluster_token {
            Some(t) => t,
            None => return Ok(()), // no token configured — open access
        };
        let provided = request_meta
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if provided == expected {
            Ok(())
        } else {
            Err(Status::unauthenticated("Invalid or missing cluster token"))
        }
    }
}

/// Dispatch a test to all connected workers
pub async fn dispatch_test_to_workers(
    workers: &WorkerRegistry,
    script_content: String,
    config_json: String,
    assets: HashMap<String, String>,
    metrics_url: String,
) -> Result<usize, String> {
    let workers_guard = workers.read().map_err(|e| e.to_string())?;
    let worker_count = workers_guard.len();

    if worker_count == 0 {
        return Err("No workers connected".to_string());
    }

    let cmd = ControllerCommand {
        cmd: Some(controller_command::Cmd::StartTest(StartTest {
            script_content,
            config_json,
            assets,
            metrics_url,
        })),
    };

    let mut sent_count = 0;
    for (id, worker) in workers_guard.iter() {
        match worker.sender.try_send(Ok(cmd.clone())) {
            Ok(_) => {
                println!("Dispatched test to worker {}", id);
                sent_count += 1;
            }
            Err(e) => {
                eprintln!("Failed to dispatch to worker {}: {}", id, e);
            }
        }
    }

    Ok(sent_count)
}

#[tonic::async_trait]
impl Cluster for ClusterService {
    type RegisterStream = Pin<Box<dyn Stream<Item = Result<ControllerCommand, Status>> + Send>>;

    async fn register(
        &self,
        request: Request<Streaming<WorkerMessage>>,
    ) -> Result<Response<Self::RegisterStream>, Status> {
        self.check_auth(request.metadata())?;
        let mut stream = request.into_inner();
        let workers = self.workers.clone();

        // Create a channel for sending commands to this worker
        let (tx, rx) = mpsc::channel(100);

        // Spawn a task to handle ALL messages (including initial registration)
        // This avoids deadlock: client can't send until we return, we can't wait before returning
        tokio::spawn(async move {
            // First message must be registration
            let first_msg = match stream.message().await {
                Ok(Some(msg)) => msg,
                Ok(None) => {
                    eprintln!("Worker stream closed before registration");
                    return;
                }
                Err(e) => {
                    eprintln!("Failed to receive registration: {}", e);
                    return;
                }
            };

            let worker_id = match first_msg.msg {
                Some(worker_message::Msg::Register(reg)) => {
                    let worker_info = WorkerInfo {
                        id: reg.id.clone(),
                        address: reg.address.clone(),
                        available_cpus: reg.available_cpus,
                        sender: tx.clone(),
                    };

                    println!(
                        "Worker registered: {} (CPUs: {})",
                        reg.id, reg.available_cpus
                    );

                    // Store worker in registry
                    if let Ok(mut guard) = workers.write() {
                        guard.insert(reg.id.clone(), worker_info);
                    }

                    reg.id
                }
                _ => {
                    eprintln!("First message must be Register");
                    return;
                }
            };

            // Handle subsequent messages (Heartbeats, TestFinished, etc.)
            while let Ok(Some(msg)) = stream.message().await {
                match msg.msg {
                    Some(worker_message::Msg::Heartbeat(hb)) => {
                        println!("Heartbeat from worker {}", hb.worker_id);
                    }
                    Some(worker_message::Msg::TestFinished(tf)) => {
                        println!("Test finished on worker {}", tf.worker_id);
                    }
                    _ => {}
                }
            }

            // Worker disconnected - remove from registry
            println!("Worker {} disconnected", worker_id);
            if let Ok(mut guard) = workers.write() {
                guard.remove(&worker_id);
            }
        });

        // Return immediately with the output stream
        let output_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::RegisterStream
        ))
    }

    async fn report_metrics(
        &self,
        request: Request<Streaming<MetricBatch>>,
    ) -> Result<Response<Empty>, Status> {
        self.check_auth(request.metadata())?;
        let mut stream = request.into_inner();
        while let Ok(Some(batch)) = stream.message().await {
            if let Ok(mut guard) = self.aggregator.write() {
                for metric in batch.metrics {
                    let m = InternalMetric::Request {
                        name: metric.name,
                        timings: crate::stats::RequestTimings {
                            duration: Duration::from_nanos(metric.duration_ns),
                            blocked: Duration::from_nanos(metric.blocked_ns),
                            connecting: Duration::from_nanos(metric.connecting_ns),
                            tls_handshaking: Duration::from_nanos(metric.tls_ns),
                            sending: Duration::from_nanos(metric.sending_ns),
                            waiting: Duration::from_nanos(metric.waiting_ns),
                            receiving: Duration::from_nanos(metric.receiving_ns),
                            response_size: metric.response_size as usize,
                            request_size: metric.request_size as usize,
                            pool_reused: false,
                        },
                        status: metric.status as u16,
                        error: metric.error,
                        tags: std::collections::HashMap::new(),
                    };
                    guard.add(m);
                }
            }
        }
        Ok(Response::new(Empty {}))
    }
}

/// Convert an internal metric to a protobuf Metric for sending to the controller.
pub fn internal_metric_to_proto(m: &InternalMetric) -> Option<proto::Metric> {
    match m {
        InternalMetric::Request {
            name,
            timings,
            status,
            error,
            ..
        } => Some(proto::Metric {
            name: name.clone(),
            duration_ns: timings.duration.as_nanos() as u64,
            status: *status as u32,
            error: error.clone(),
            timestamp: 0,
            blocked_ns: timings.blocked.as_nanos() as u64,
            connecting_ns: timings.connecting.as_nanos() as u64,
            tls_ns: timings.tls_handshaking.as_nanos() as u64,
            sending_ns: timings.sending.as_nanos() as u64,
            waiting_ns: timings.waiting.as_nanos() as u64,
            receiving_ns: timings.receiving.as_nanos() as u64,
            response_size: timings.response_size as u64,
            request_size: timings.request_size as u64,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_registry() -> WorkerRegistry {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn make_worker(id: &str) -> WorkerInfo {
        let (tx, _rx) = mpsc::channel(10);
        WorkerInfo {
            id: id.to_string(),
            address: format!("127.0.0.1:500{}", id),
            available_cpus: 4,
            sender: tx,
        }
    }

    #[test]
    fn test_worker_registry_add_remove() {
        let registry = make_registry();
        let worker = make_worker("1");

        {
            let mut guard = registry.write().unwrap();
            guard.insert("w1".to_string(), worker);
        }

        {
            let guard = registry.read().unwrap();
            assert_eq!(guard.len(), 1);
            assert!(guard.contains_key("w1"));
            assert_eq!(guard["w1"].available_cpus, 4);
        }

        {
            let mut guard = registry.write().unwrap();
            guard.remove("w1");
        }

        {
            let guard = registry.read().unwrap();
            assert!(guard.is_empty());
        }
    }

    #[test]
    fn test_worker_registry_multiple_workers() {
        let registry = make_registry();

        {
            let mut guard = registry.write().unwrap();
            guard.insert("w1".to_string(), make_worker("1"));
            guard.insert("w2".to_string(), make_worker("2"));
            guard.insert("w3".to_string(), make_worker("3"));
        }

        let guard = registry.read().unwrap();
        assert_eq!(guard.len(), 3);
    }

    #[tokio::test]
    async fn test_dispatch_no_workers() {
        let registry = make_registry();
        let result = dispatch_test_to_workers(
            &registry,
            "script".to_string(),
            "{}".to_string(),
            HashMap::new(),
            "http://localhost:9090".to_string(),
        )
        .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "No workers connected");
    }

    #[tokio::test]
    async fn test_dispatch_to_workers() {
        let registry = make_registry();
        let (tx1, mut rx1) = mpsc::channel(10);
        let (tx2, mut rx2) = mpsc::channel(10);

        {
            let mut guard = registry.write().unwrap();
            guard.insert(
                "w1".to_string(),
                WorkerInfo {
                    id: "w1".to_string(),
                    address: "127.0.0.1:5001".to_string(),
                    available_cpus: 4,
                    sender: tx1,
                },
            );
            guard.insert(
                "w2".to_string(),
                WorkerInfo {
                    id: "w2".to_string(),
                    address: "127.0.0.1:5002".to_string(),
                    available_cpus: 8,
                    sender: tx2,
                },
            );
        }

        let result = dispatch_test_to_workers(
            &registry,
            "export default function() {}".to_string(),
            "{\"workers\": 10}".to_string(),
            HashMap::new(),
            "http://controller:9090".to_string(),
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2);

        // Both workers should have received the command
        let cmd1 = rx1.recv().await.unwrap().unwrap();
        let cmd2 = rx2.recv().await.unwrap().unwrap();

        match cmd1.cmd.unwrap() {
            controller_command::Cmd::StartTest(st) => {
                assert_eq!(st.script_content, "export default function() {}");
                assert_eq!(st.config_json, "{\"workers\": 10}");
                assert_eq!(st.metrics_url, "http://controller:9090");
            }
            _ => panic!("Expected StartTest command"),
        }

        match cmd2.cmd.unwrap() {
            controller_command::Cmd::StartTest(st) => {
                assert_eq!(st.script_content, "export default function() {}");
            }
            _ => panic!("Expected StartTest command"),
        }
    }

    #[test]
    fn test_metric_conversion_all_timing_fields() {
        let proto_metric = proto::Metric {
            name: "GET /api/users".to_string(),
            duration_ns: 150_000_000,
            status: 200,
            error: None,
            timestamp: 1234567890,
            blocked_ns: 1_000_000,
            connecting_ns: 5_000_000,
            tls_ns: 10_000_000,
            sending_ns: 2_000_000,
            waiting_ns: 120_000_000,
            receiving_ns: 12_000_000,
            response_size: 4096,
            request_size: 256,
        };

        let internal = InternalMetric::Request {
            name: proto_metric.name.clone(),
            timings: crate::stats::RequestTimings {
                duration: Duration::from_nanos(proto_metric.duration_ns),
                blocked: Duration::from_nanos(proto_metric.blocked_ns),
                connecting: Duration::from_nanos(proto_metric.connecting_ns),
                tls_handshaking: Duration::from_nanos(proto_metric.tls_ns),
                sending: Duration::from_nanos(proto_metric.sending_ns),
                waiting: Duration::from_nanos(proto_metric.waiting_ns),
                receiving: Duration::from_nanos(proto_metric.receiving_ns),
                response_size: proto_metric.response_size as usize,
                request_size: proto_metric.request_size as usize,
                pool_reused: false,
            },
            status: proto_metric.status as u16,
            error: proto_metric.error.clone(),
            tags: std::collections::HashMap::new(),
        };

        if let InternalMetric::Request {
            name,
            timings,
            status,
            error,
            ..
        } = &internal
        {
            assert_eq!(name, "GET /api/users");
            assert_eq!(timings.duration, Duration::from_millis(150));
            assert_eq!(timings.blocked, Duration::from_millis(1));
            assert_eq!(timings.connecting, Duration::from_millis(5));
            assert_eq!(timings.tls_handshaking, Duration::from_millis(10));
            assert_eq!(timings.sending, Duration::from_millis(2));
            assert_eq!(timings.waiting, Duration::from_millis(120));
            assert_eq!(timings.receiving, Duration::from_millis(12));
            assert_eq!(timings.response_size, 4096);
            assert_eq!(timings.request_size, 256);
            assert_eq!(*status, 200);
            assert!(error.is_none());
        } else {
            panic!("Expected Request metric");
        }
    }

    #[test]
    fn test_metric_conversion_zero_fields() {
        let proto_metric = proto::Metric {
            name: "GET /health".to_string(),
            duration_ns: 0,
            status: 200,
            error: None,
            timestamp: 0,
            blocked_ns: 0,
            connecting_ns: 0,
            tls_ns: 0,
            sending_ns: 0,
            waiting_ns: 0,
            receiving_ns: 0,
            response_size: 0,
            request_size: 0,
        };

        let internal = InternalMetric::Request {
            name: proto_metric.name.clone(),
            timings: crate::stats::RequestTimings {
                duration: Duration::from_nanos(proto_metric.duration_ns),
                blocked: Duration::from_nanos(proto_metric.blocked_ns),
                connecting: Duration::from_nanos(proto_metric.connecting_ns),
                tls_handshaking: Duration::from_nanos(proto_metric.tls_ns),
                sending: Duration::from_nanos(proto_metric.sending_ns),
                waiting: Duration::from_nanos(proto_metric.waiting_ns),
                receiving: Duration::from_nanos(proto_metric.receiving_ns),
                response_size: proto_metric.response_size as usize,
                request_size: proto_metric.request_size as usize,
                pool_reused: false,
            },
            status: proto_metric.status as u16,
            error: proto_metric.error.clone(),
            tags: std::collections::HashMap::new(),
        };

        if let InternalMetric::Request { timings, .. } = &internal {
            assert_eq!(timings.duration, Duration::ZERO);
            assert_eq!(timings.blocked, Duration::ZERO);
            assert_eq!(timings.connecting, Duration::ZERO);
            assert_eq!(timings.tls_handshaking, Duration::ZERO);
            assert_eq!(timings.sending, Duration::ZERO);
            assert_eq!(timings.waiting, Duration::ZERO);
            assert_eq!(timings.receiving, Duration::ZERO);
            assert_eq!(timings.response_size, 0);
            assert_eq!(timings.request_size, 0);
        } else {
            panic!("Expected Request metric");
        }
    }

    #[test]
    fn test_metric_with_error() {
        let proto_metric = proto::Metric {
            name: "GET /fail".to_string(),
            duration_ns: 5_000_000_000,
            status: 500,
            error: Some("Connection refused".to_string()),
            timestamp: 0,
            blocked_ns: 0,
            connecting_ns: 0,
            tls_ns: 0,
            sending_ns: 0,
            waiting_ns: 0,
            receiving_ns: 0,
            response_size: 0,
            request_size: 0,
        };

        let internal = InternalMetric::Request {
            name: proto_metric.name.clone(),
            timings: crate::stats::RequestTimings {
                duration: Duration::from_nanos(proto_metric.duration_ns),
                blocked: Duration::from_nanos(proto_metric.blocked_ns),
                connecting: Duration::from_nanos(proto_metric.connecting_ns),
                tls_handshaking: Duration::from_nanos(proto_metric.tls_ns),
                sending: Duration::from_nanos(proto_metric.sending_ns),
                waiting: Duration::from_nanos(proto_metric.waiting_ns),
                receiving: Duration::from_nanos(proto_metric.receiving_ns),
                response_size: proto_metric.response_size as usize,
                request_size: proto_metric.request_size as usize,
                pool_reused: false,
            },
            status: proto_metric.status as u16,
            error: proto_metric.error.clone(),
            tags: std::collections::HashMap::new(),
        };

        if let InternalMetric::Request {
            status,
            error,
            timings,
            ..
        } = &internal
        {
            assert_eq!(*status, 500);
            assert_eq!(error.as_deref(), Some("Connection refused"));
            assert_eq!(timings.duration, Duration::from_secs(5));
        } else {
            panic!("Expected Request metric");
        }
    }

    #[test]
    fn test_internal_metric_to_proto_roundtrip() {
        let original = InternalMetric::Request {
            name: "POST /api/data".to_string(),
            timings: crate::stats::RequestTimings {
                duration: Duration::from_millis(250),
                blocked: Duration::from_millis(1),
                connecting: Duration::from_millis(10),
                tls_handshaking: Duration::from_millis(20),
                sending: Duration::from_millis(5),
                waiting: Duration::from_millis(200),
                receiving: Duration::from_millis(14),
                response_size: 8192,
                request_size: 512,
                pool_reused: true,
            },
            status: 201,
            error: None,
            tags: std::collections::HashMap::new(),
        };

        let proto = internal_metric_to_proto(&original).unwrap();
        assert_eq!(proto.name, "POST /api/data");
        assert_eq!(proto.duration_ns, 250_000_000);
        assert_eq!(proto.blocked_ns, 1_000_000);
        assert_eq!(proto.connecting_ns, 10_000_000);
        assert_eq!(proto.tls_ns, 20_000_000);
        assert_eq!(proto.sending_ns, 5_000_000);
        assert_eq!(proto.waiting_ns, 200_000_000);
        assert_eq!(proto.receiving_ns, 14_000_000);
        assert_eq!(proto.response_size, 8192);
        assert_eq!(proto.request_size, 512);
        assert_eq!(proto.status, 201);
        assert!(proto.error.is_none());
    }

    #[test]
    fn test_internal_metric_to_proto_check_returns_none() {
        let check = InternalMetric::Check {
            name: "status is 200".to_string(),
            success: true,
            message: None,
        };
        assert!(internal_metric_to_proto(&check).is_none());
    }

    #[test]
    fn test_cluster_service_new() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let service = ClusterService::new(aggregator);
        let workers = service.workers();
        assert!(workers.read().unwrap().is_empty());
    }

    #[test]
    fn test_cluster_service_with_registry() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let registry = make_registry();

        {
            let mut guard = registry.write().unwrap();
            guard.insert("w1".to_string(), make_worker("1"));
        }

        let service = ClusterService::new_with_registry(aggregator, registry, None);
        let workers = service.workers();
        assert_eq!(workers.read().unwrap().len(), 1);
    }

    #[test]
    fn test_check_auth_no_token_allows_all() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let service = ClusterService::new(aggregator);
        let meta = tonic::metadata::MetadataMap::new();
        assert!(service.check_auth(&meta).is_ok());
    }

    #[test]
    fn test_check_auth_correct_token_allowed() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let service = ClusterService::new_with_registry(
            Arc::new(std::sync::RwLock::new(Default::default())),
            make_registry(),
            Some("secret123".to_string()),
        );
        let mut meta = tonic::metadata::MetadataMap::new();
        meta.insert("authorization", "Bearer secret123".parse().unwrap());
        assert!(service.check_auth(&meta).is_ok());
    }

    #[test]
    fn test_check_auth_wrong_token_rejected() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let service = ClusterService::new_with_registry(
            aggregator,
            make_registry(),
            Some("secret123".to_string()),
        );
        let mut meta = tonic::metadata::MetadataMap::new();
        meta.insert("authorization", "Bearer wrongtoken".parse().unwrap());
        assert!(service.check_auth(&meta).is_err());
    }

    #[test]
    fn test_check_auth_missing_token_rejected() {
        let aggregator: SharedAggregator = Arc::new(std::sync::RwLock::new(
            crate::stats::StatsAggregator::default(),
        ));
        let service = ClusterService::new_with_registry(
            aggregator,
            make_registry(),
            Some("secret123".to_string()),
        );
        let meta = tonic::metadata::MetadataMap::new();
        assert!(service.check_auth(&meta).is_err());
    }
}
