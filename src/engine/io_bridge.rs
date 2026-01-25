//! I/O Bridge for Green Threads
//!
//! This module provides a bridge between may coroutines and Tokio async I/O.
//! Coroutines send HTTP requests via channel and yield until response arrives.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use http::{Request, Response};
use hyper::body::Bytes;
use tokio::runtime::Runtime;

use crate::engine::http_client::HttpClient;
use crate::stats::RequestTimings;

/// Request payload sent from coroutine to Tokio I/O pool
pub struct IoRequest {
    pub request: Request<String>,
    pub timeout: Option<Duration>,
    pub response_tx: may::sync::mpsc::Sender<IoResponse>,
}

/// Response payload sent back to coroutine
pub struct IoResponse {
    pub result: Result<(Response<Bytes>, RequestTimings), String>,
}

/// I/O Bridge that forwards requests from may coroutines to Tokio
pub struct IoBridge {
    request_tx: Sender<IoRequest>,
    _worker_handles: Vec<std::thread::JoinHandle<()>>,
}

impl IoBridge {
    /// Create a new I/O bridge with specified number of Tokio worker threads
    pub fn new(
        tokio_rt: Arc<Runtime>,
        http_client: Arc<HttpClient>,
        num_io_workers: usize,
    ) -> Self {
        // Bounded channel to prevent unbounded memory growth
        // Size: 10k to handle burst of requests from coroutines
        let (request_tx, request_rx): (Sender<IoRequest>, Receiver<IoRequest>) = bounded(10_000);

        let mut worker_handles = Vec::with_capacity(num_io_workers);

        // Spawn multiple I/O worker threads that process requests on Tokio
        for worker_id in 0..num_io_workers {
            let rx = request_rx.clone();
            let rt = tokio_rt.clone();
            let client = http_client.clone();

            let handle = std::thread::Builder::new()
                .name(format!("io-bridge-{}", worker_id))
                .spawn(move || {
                    // Each worker runs in its own thread, using the shared Tokio runtime
                    rt.block_on(async move {
                        while let Ok(io_req) = rx.recv() {
                            let IoRequest {
                                request,
                                timeout,
                                response_tx,
                            } = io_req;

                            // Execute HTTP request
                            // I/O bridge doesn't currently support response_sink, use false
                            let result = if let Some(timeout_dur) = timeout {
                                match tokio::time::timeout(
                                    timeout_dur,
                                    client.request(request, false),
                                )
                                .await
                                {
                                    Ok(res) => res.map_err(|e| e.to_string()),
                                    Err(_) => Err("Request timeout".to_string()),
                                }
                            } else {
                                client
                                    .request(request, false)
                                    .await
                                    .map_err(|e| e.to_string())
                            };

                            // Send response back to coroutine (non-blocking for Tokio side)
                            let _ = response_tx.send(IoResponse { result });
                        }
                    });
                })
                .expect("Failed to spawn I/O bridge worker");

            worker_handles.push(handle);
        }

        Self {
            request_tx,
            _worker_handles: worker_handles,
        }
    }

    /// Send an HTTP request from a may coroutine.
    /// This yields the coroutine until the response is ready.
    pub fn request(
        &self,
        request: Request<String>,
        timeout: Option<Duration>,
    ) -> Result<(Response<Bytes>, RequestTimings), String> {
        // Create a channel for this specific request
        let (response_tx, response_rx) = may::sync::mpsc::channel();

        let io_req = IoRequest {
            request,
            timeout,
            response_tx,
        };

        // Send request to I/O pool
        match self.request_tx.try_send(io_req) {
            Ok(_) => {}
            Err(TrySendError::Full(req)) => {
                // Channel full - apply backpressure by blocking send
                if self.request_tx.send(req).is_err() {
                    return Err("I/O bridge channel closed".to_string());
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                return Err("I/O bridge channel disconnected".to_string());
            }
        }

        // Wait for response - this yields the may coroutine
        match response_rx.recv() {
            Ok(response) => response.result,
            Err(_) => Err("Response channel closed".to_string()),
        }
    }

    /// Send request with timing measurement
    pub fn request_timed(
        &self,
        request: Request<String>,
        timeout: Option<Duration>,
    ) -> (Result<(Response<Bytes>, RequestTimings), String>, Duration) {
        let start = Instant::now();
        let result = self.request(request, timeout);
        let elapsed = start.elapsed();
        (result, elapsed)
    }
}

impl Drop for IoBridge {
    fn drop(&mut self) {
        // Dropping request_tx will cause workers to exit when channel is empty
        // Worker threads will join when IoBridge is fully dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_bridge_creation() {
        // This test just verifies the bridge can be created
        // Full integration tests require may coroutine context
        let _ = rustls::crypto::ring::default_provider().install_default();
        let rt = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = Arc::new(HttpClient::new());
        let _bridge = IoBridge::new(rt, client, 2);
    }
}
