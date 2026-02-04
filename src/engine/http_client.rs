use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use http::{Request, Response, Uri};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tower::Service;

use crate::stats::RequestTimings;

// Task-local timing storage for per-request connection measurements.
// Each tokio task gets its own timing slots, eliminating races between
// concurrent requests sharing the same HttpClient.
tokio::task_local! {
    static TASK_CONNECTING_NANOS: Cell<u64>;
    static TASK_TLS_NANOS: Cell<u64>;
}

/// Record TCP connect time into the current task's timing slot.
/// No-op if called outside a task-local scope (pooled ureq path).
fn record_connecting(d: Duration) {
    let _ = TASK_CONNECTING_NANOS.try_with(|cell| {
        cell.set(cell.get() + d.as_nanos() as u64);
    });
}

/// Record TLS handshake time into the current task's timing slot.
fn record_tls(d: Duration) {
    let _ = TASK_TLS_NANOS.try_with(|cell| {
        cell.set(cell.get() + d.as_nanos() as u64);
    });
}

/// Read and consume the per-task connecting time.
fn take_connecting() -> Duration {
    TASK_CONNECTING_NANOS
        .try_with(|cell| {
            let nanos = cell.get();
            cell.set(0);
            Duration::from_nanos(nanos)
        })
        .unwrap_or(Duration::ZERO)
}

/// Read and consume the per-task TLS time.
fn take_tls() -> Duration {
    TASK_TLS_NANOS
        .try_with(|cell| {
            let nanos = cell.get();
            cell.set(0);
            Duration::from_nanos(nanos)
        })
        .unwrap_or(Duration::ZERO)
}

// 1. Wrapped HttpConnector to measure TCP/DNS time
#[derive(Clone)]
struct MeasuredHttpConnector {
    inner: HttpConnector,
}

impl MeasuredHttpConnector {
    pub fn new() -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        Self { inner: http }
    }
}

impl Service<Uri> for MeasuredHttpConnector {
    type Response = <HttpConnector as Service<Uri>>::Response;
    type Error = <HttpConnector as Service<Uri>>::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let mut inner = self.inner.clone();
        let start = Instant::now();

        Box::pin(async move {
            let res = inner.call(uri).await;
            record_connecting(start.elapsed());
            res
        })
    }
}

// 2. Connector Stack that measures TLS on top of TCP
#[derive(Clone)]
struct MeasuredHttpsConnector {
    inner: HttpsConnector<MeasuredHttpConnector>,
}

impl MeasuredHttpsConnector {
    pub fn new(http: MeasuredHttpConnector) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .unwrap()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http);

        Self { inner: https }
    }
}

impl Service<Uri> for MeasuredHttpsConnector {
    type Response = <HttpsConnector<MeasuredHttpConnector> as Service<Uri>>::Response;
    type Error = <HttpsConnector<MeasuredHttpConnector> as Service<Uri>>::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let mut inner = self.inner.clone();
        let start = Instant::now();

        Box::pin(async move {
            let res = inner.call(uri).await;
            // TLS time = total HTTPS connector time minus the inner TCP time already recorded
            record_tls(start.elapsed());
            res
        })
    }
}

#[derive(Clone)]
pub struct HttpClient {
    client: Client<MeasuredHttpsConnector, Full<Bytes>>,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self::with_pool_size(2000)
    }

    /// Create a new HttpClient with a custom connection pool size
    /// pool_size: Maximum idle connections per host (default: 2000)
    pub fn with_pool_size(pool_size: usize) -> Self {
        Self::with_pool_and_workers(pool_size, 1000) // Default to 1K worker config
    }

    /// Create a new HttpClient with connection pooling disabled.
    /// Every request establishes a new TCP connection and TLS handshake.
    pub fn with_no_pool(total_workers: usize) -> Self {
        let http = MeasuredHttpConnector::new();
        let https = MeasuredHttpsConnector::new(http);

        let (conn_window, stream_window) = if total_workers > 5000 {
            (128 * 1024, 64 * 1024)
        } else if total_workers > 2000 {
            (256 * 1024, 128 * 1024)
        } else {
            (512 * 1024, 256 * 1024)
        };

        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::ZERO)
            .pool_max_idle_per_host(0)
            .http2_initial_connection_window_size(conn_window)
            .http2_initial_stream_window_size(stream_window)
            .build(https);

        Self { client }
    }

    /// Create a new HttpClient with pool size and worker-scaled HTTP/2 windows
    /// At high worker counts (>5K), use smaller windows to reduce memory
    pub fn with_pool_and_workers(pool_size: usize, total_workers: usize) -> Self {
        let http = MeasuredHttpConnector::new();
        let https = MeasuredHttpsConnector::new(http);

        // Scale HTTP/2 windows based on worker count:
        // - Low workers (<2K): larger windows for better throughput
        // - High workers (>5K): smaller windows to save memory
        let (conn_window, stream_window) = if total_workers > 5000 {
            (128 * 1024, 64 * 1024) // 128KB/64KB for high concurrency
        } else if total_workers > 2000 {
            (256 * 1024, 128 * 1024) // 256KB/128KB for medium concurrency
        } else {
            (512 * 1024, 256 * 1024) // 512KB/256KB for low concurrency
        };

        let client = Client::builder(TokioExecutor::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(pool_size)
            .http2_initial_connection_window_size(conn_window)
            .http2_initial_stream_window_size(stream_window)
            .build(https);

        Self { client }
    }

    /// Make an HTTP request. If `response_sink` is true, the response body is read from the network
    /// (required for connection keep-alive) but immediately discarded to save memory.
    /// The returned Response will have an empty body when sink mode is enabled.
    ///
    /// Must be called inside a TASK_CONNECTING_NANOS / TASK_TLS_NANOS scope
    /// (set up by `request_with_timings`) for accurate per-request connection timing.
    // Must be called inside a Tokio Runtime
    pub async fn request(
        &self,
        req: Request<String>,
        response_sink: bool,
    ) -> Result<(Response<Bytes>, RequestTimings), Box<dyn std::error::Error + Send + Sync>> {
        let request_start = Instant::now();

        let url = req.uri().clone();
        let method = req.method().clone();
        let body_str = req.body().clone();

        // Calculate approx request size
        let mut req_size = body_str.len();
        req_size += method.as_str().len() + 1 + url.to_string().len() + 11;
        for (k, v) in req.headers() {
            req_size += k.as_str().len() + 2 + v.len() + 2;
        }
        req_size += 2;

        use http_body_util::Full;
        let body_bytes = Bytes::from(body_str);
        let mut builder = Request::builder().uri(url).method(method);

        for (k, v) in req.headers() {
            builder = builder.header(k, v);
        }

        let req_hyper = builder.body(Full::new(body_bytes))?;

        let response = self.client.request(req_hyper).await?;
        let headers_received = Instant::now();

        // Split response to consume body
        let (parts, body_stream) = response.into_parts();

        // Always read the body stream to completion (required for connection keep-alive/reuse)
        // When response_sink is enabled, we discard the bytes immediately to save memory
        let (body, resp_body_size) = if response_sink {
            // Sink mode: read and discard body chunks, only track size
            let mut total_size = 0usize;
            let mut stream = body_stream;
            while let Some(chunk) = stream.frame().await {
                if let Ok(frame) = chunk {
                    if let Some(data) = frame.data_ref() {
                        total_size += data.len();
                    }
                }
            }
            (Bytes::new(), total_size)
        } else {
            // Normal mode: collect body into memory
            let body = body_stream.collect().await?.to_bytes();
            let size = body.len();
            (body, size)
        };
        let receive_end = Instant::now();

        let mut timings = RequestTimings::default();

        // Total time from request start to headers received
        let time_to_headers = headers_received.duration_since(request_start);

        timings.blocked = Duration::ZERO;
        // Read per-request connector timings from task-local storage.
        // For pooled connections these will be zero (no connector was invoked).
        timings.connecting = take_connecting();
        timings.tls_handshaking = take_tls();
        // TLS connector wraps HTTP connector, so tls time includes connecting time.
        // Subtract to get pure TLS handshake time.
        timings.tls_handshaking = timings.tls_handshaking.saturating_sub(timings.connecting);
        let connection_time = timings.connecting + timings.tls_handshaking;
        timings.sending = Duration::from_micros(100);
        timings.waiting = time_to_headers
            .saturating_sub(connection_time)
            .saturating_sub(timings.sending);
        timings.receiving = receive_end.duration_since(headers_received);

        // Total request duration: connect + send + wait + receive
        timings.duration = connection_time + timings.sending + timings.waiting + timings.receiving;

        // Calculate response size (headers + body)
        // Use resp_body_size which tracks actual bytes read (works for both normal and sink mode)
        let mut resp_size = resp_body_size;
        // Status line: HTTP/1.1 STATUS REASON\r\n (approx 15 chars + reason)
        resp_size += 15;
        for (k, v) in parts.headers.iter() {
            resp_size += k.as_str().len() + 2 + v.len() + 2;
        }
        resp_size += 2; // Final \r\n

        timings.response_size = resp_size;
        timings.request_size = req_size;
        // Pool reuse: if no connecting or TLS time was recorded, the connection was reused
        timings.pool_reused = connection_time == Duration::ZERO;

        // Construct new response with bytes body
        let mut builder = Response::builder()
            .status(parts.status)
            .version(parts.version);

        for (k, v) in parts.headers {
            if let Some(key) = k {
                builder = builder.header(key, v);
            }
        }

        let final_res = builder.body(body)?;

        Ok((final_res, timings))
    }

    /// Execute a request within a task-local timing scope.
    /// This sets up per-request timing storage so the MeasuredConnectors
    /// record connection times to this specific request, not a shared context.
    pub async fn request_with_timings(
        &self,
        req: Request<String>,
        response_sink: bool,
    ) -> Result<(Response<Bytes>, RequestTimings), Box<dyn std::error::Error + Send + Sync>> {
        TASK_CONNECTING_NANOS
            .scope(Cell::new(0), async {
                TASK_TLS_NANOS
                    .scope(Cell::new(0), self.request(req, response_sink))
                    .await
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_defaults() {
        // Just verify we can instantiate without panic
        let _path_client = HttpClient::new();
    }

    #[test]
    fn test_http_client_custom_pool() {
        // Just verify builder logic runs
        let _client = HttpClient::with_pool_and_workers(50, 100);
    }

    #[test]
    fn test_http_client_no_pool() {
        // Verify no-pool construction doesn't panic
        let _client = HttpClient::with_no_pool(100);
    }
}
