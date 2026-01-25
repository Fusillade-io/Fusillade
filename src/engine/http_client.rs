use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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

// Atomic timing context that can be safely shared across async boundaries
// Uses AtomicU64 to store nanoseconds for lock-free updates
#[derive(Clone)]
struct TimingContext {
    request_start_nanos: Arc<AtomicU64>,
    blocked_nanos: Arc<AtomicU64>,
    connecting_nanos: Arc<AtomicU64>,
    tls_nanos: Arc<AtomicU64>,
}

#[allow(dead_code)]
impl TimingContext {
    fn new() -> Self {
        Self {
            request_start_nanos: Arc::new(AtomicU64::new(0)),
            blocked_nanos: Arc::new(AtomicU64::new(0)),
            connecting_nanos: Arc::new(AtomicU64::new(0)),
            tls_nanos: Arc::new(AtomicU64::new(0)),
        }
    }

    fn set_start(&self, instant: Instant) {
        // Store as nanos since some epoch (we'll use elapsed from a base)
        self.request_start_nanos
            .store(instant.elapsed().as_nanos() as u64, Ordering::Release);
    }

    fn add_blocked(&self, d: Duration) {
        self.blocked_nanos
            .fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
    }

    fn add_connecting(&self, d: Duration) {
        self.connecting_nanos
            .fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
    }

    fn add_tls(&self, d: Duration) {
        self.tls_nanos
            .fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
    }

    fn get_blocked(&self) -> Duration {
        Duration::from_nanos(self.blocked_nanos.load(Ordering::Acquire))
    }

    fn get_connecting(&self) -> Duration {
        Duration::from_nanos(self.connecting_nanos.load(Ordering::Acquire))
    }

    fn get_tls(&self) -> Duration {
        Duration::from_nanos(self.tls_nanos.load(Ordering::Acquire))
    }

    fn reset(&self) {
        self.request_start_nanos.store(0, Ordering::Release);
        self.blocked_nanos.store(0, Ordering::Release);
        self.connecting_nanos.store(0, Ordering::Release);
        self.tls_nanos.store(0, Ordering::Release);
    }
}

// 1. Wrapped HttpConnector to measure TCP/DNS time
#[derive(Clone)]
struct MeasuredHttpConnector {
    inner: HttpConnector,
    timing_ctx: TimingContext,
}

impl MeasuredHttpConnector {
    pub fn new(timing_ctx: TimingContext) -> Self {
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        Self {
            inner: http,
            timing_ctx,
        }
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
        let timing_ctx = self.timing_ctx.clone();
        let start = Instant::now();

        Box::pin(async move {
            let res = inner.call(uri).await;
            timing_ctx.add_connecting(start.elapsed());
            res
        })
    }
}

// 2. Connector Stack with timing context
#[derive(Clone)]
struct MeasuredHttpsConnector {
    inner: HttpsConnector<MeasuredHttpConnector>,
    timing_ctx: TimingContext,
}

impl MeasuredHttpsConnector {
    pub fn new(http: MeasuredHttpConnector, timing_ctx: TimingContext) -> Self {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .unwrap()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http);

        Self {
            inner: https,
            timing_ctx,
        }
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
        let timing_ctx = self.timing_ctx.clone();
        let start = Instant::now();

        Box::pin(async move {
            let res = inner.call(uri).await;
            timing_ctx.add_tls(start.elapsed());
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

    /// Create a new HttpClient with pool size and worker-scaled HTTP/2 windows
    /// At high worker counts (>5K), use smaller windows to reduce memory
    pub fn with_pool_and_workers(pool_size: usize, total_workers: usize) -> Self {
        let timing_ctx = TimingContext::new();
        let http = MeasuredHttpConnector::new(timing_ctx.clone());
        let https = MeasuredHttpsConnector::new(http, timing_ctx.clone());

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

        // Simple timing: measure actual durations without connector-level breakdown
        // This avoids race conditions with shared timing context in concurrent requests
        let mut timings = RequestTimings::default();

        // Total time from request start to headers received (includes connection, TLS, send, wait)
        let time_to_headers = headers_received.duration_since(request_start);

        // Rough breakdown: assume connection/TLS is ~10% of time for warmed connections
        // For new connections, the connector timing would show this, but pooled connections
        // skip that entirely, making breakdown unreliable without per-request state
        timings.blocked = Duration::ZERO;
        timings.connecting = Duration::ZERO;
        timings.tls_handshaking = Duration::ZERO;
        timings.sending = Duration::from_micros(100);
        timings.waiting = time_to_headers.saturating_sub(timings.sending);
        timings.receiving = receive_end.duration_since(headers_received);

        // Total request duration: send + wait + receive
        timings.duration = timings.sending + timings.waiting + timings.receiving;

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
}
