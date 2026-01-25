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
}

impl ClusterService {
    pub fn new(aggregator: SharedAggregator) -> Self {
        Self {
            aggregator,
            workers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn new_with_registry(aggregator: SharedAggregator, workers: WorkerRegistry) -> Self {
        Self {
            aggregator,
            workers,
        }
    }

    pub fn workers(&self) -> WorkerRegistry {
        self.workers.clone()
    }
}

/// Dispatch a test to all connected workers
pub async fn dispatch_test_to_workers(
    workers: &WorkerRegistry,
    script_content: String,
    config_json: String,
    assets: HashMap<String, String>,
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
        let mut stream = request.into_inner();
        while let Ok(Some(batch)) = stream.message().await {
            if let Ok(mut guard) = self.aggregator.write() {
                for metric in batch.metrics {
                    let m = InternalMetric::Request {
                        name: metric.name,
                        timings: crate::stats::RequestTimings {
                            duration: Duration::from_nanos(metric.duration_ns),
                            ..Default::default()
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
