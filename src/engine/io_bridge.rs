//! I/O Bridge for Green Threads
//!
//! This module provides a bridge between may coroutines and Tokio async I/O.
//! May coroutines spawn tokio tasks directly via a runtime handle and yield
//! until the response arrives via a may channel.

use std::sync::Arc;
use std::time::{Duration, Instant};

use http::{Request, Response};
use hyper::body::Bytes;
use tokio::runtime::Handle;

use crate::engine::http_client::HttpClient;
use crate::stats::RequestTimings;

/// Response payload sent back to coroutine
pub struct IoResponse {
    pub result: Result<(Response<Bytes>, RequestTimings), String>,
}

/// I/O Bridge that spawns tokio tasks directly from may coroutines.
/// No intermediate channel or worker threads — minimal latency overhead.
pub struct IoBridge {
    tokio_handle: Handle,
    http_client: Arc<HttpClient>,
}

impl IoBridge {
    /// Create a new I/O bridge. The `_num_io_workers` parameter is kept for
    /// API compatibility but is no longer used — tasks are spawned directly
    /// on the shared tokio runtime.
    pub fn new(
        tokio_rt: Arc<tokio::runtime::Runtime>,
        http_client: Arc<HttpClient>,
        _num_io_workers: usize,
    ) -> Self {
        Self {
            tokio_handle: tokio_rt.handle().clone(),
            http_client,
        }
    }

    /// Send an HTTP request from a may coroutine.
    /// Spawns a tokio task and yields the coroutine until the response is ready.
    pub fn request(
        &self,
        request: Request<String>,
        timeout: Option<Duration>,
        response_sink: bool,
    ) -> Result<(Response<Bytes>, RequestTimings), String> {
        // Create a may channel for this specific request — recv() yields the coroutine
        let (response_tx, response_rx) = may::sync::mpsc::channel();

        let client = self.http_client.clone();

        // Spawn directly on the tokio runtime — no channel, no worker thread
        self.tokio_handle.spawn(async move {
            let result = if let Some(timeout_dur) = timeout {
                match tokio::time::timeout(
                    timeout_dur,
                    client.request_with_timings(request, response_sink),
                )
                .await
                {
                    Ok(res) => res.map_err(|e| e.to_string()),
                    Err(_) => Err("Request timeout".to_string()),
                }
            } else {
                client
                    .request_with_timings(request, response_sink)
                    .await
                    .map_err(|e| e.to_string())
            };

            let _ = response_tx.send(IoResponse { result });
        });

        // Yield the may coroutine until the tokio task completes
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
        response_sink: bool,
    ) -> (Result<(Response<Bytes>, RequestTimings), String>, Duration) {
        let start = Instant::now();
        let result = self.request(request, timeout, response_sink);
        let elapsed = start.elapsed();
        (result, elapsed)
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
