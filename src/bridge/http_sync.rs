//! Synchronous HTTP bridge using ureq - bypasses Tokio for lower latency
//! This module provides a drop-in replacement for http.rs that uses blocking I/O
//! directly compatible with May green threads.
//!
//! When an IoBridge is provided (no_pool mode), requests are routed through
//! Hyper with connection pooling disabled, providing per-request DNS+TCP and TLS timing.

use crate::engine::io_bridge::IoBridge;
use crate::stats::{Metric, RequestTimings};
use cookie::{Cookie, SameSite};
use crossbeam_channel::Sender;
use hyper::body::Bytes;
use rquickjs::{Array, Ctx, Function, IntoJs, Object, Result, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Map HTTP status code to status text
fn status_text_for_code(code: u16) -> String {
    match code {
        200 => "OK".to_string(),
        201 => "Created".to_string(),
        204 => "No Content".to_string(),
        301 => "Moved Permanently".to_string(),
        302 => "Found".to_string(),
        304 => "Not Modified".to_string(),
        400 => "Bad Request".to_string(),
        401 => "Unauthorized".to_string(),
        403 => "Forbidden".to_string(),
        404 => "Not Found".to_string(),
        405 => "Method Not Allowed".to_string(),
        408 => "Request Timeout".to_string(),
        429 => "Too Many Requests".to_string(),
        500 => "Internal Server Error".to_string(),
        502 => "Bad Gateway".to_string(),
        503 => "Service Unavailable".to_string(),
        504 => "Gateway Timeout".to_string(),
        0 => "Network Error".to_string(),
        _ => "".to_string(),
    }
}

/// Categorize an error string into a type and code for better error handling in JS
fn categorize_error(error: &str) -> (String, String) {
    let lower = error.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        ("TIMEOUT".to_string(), "ETIMEDOUT".to_string())
    } else if lower.contains("dns") || lower.contains("resolve") || lower.contains("getaddrinfo") {
        ("DNS".to_string(), "ENOTFOUND".to_string())
    } else if lower.contains("certificate") || lower.contains("ssl") || lower.contains("tls") {
        ("TLS".to_string(), "ECERT".to_string())
    } else if lower.contains("connection refused") {
        ("CONNECT".to_string(), "ECONNREFUSED".to_string())
    } else if lower.contains("reset") {
        ("RESET".to_string(), "ECONNRESET".to_string())
    } else if lower.contains("broken pipe") {
        ("RESET".to_string(), "EPIPE".to_string())
    } else {
        ("NETWORK".to_string(), "ENETWORK".to_string())
    }
}

/// Sync HTTP response (matches HttpResponse from http.rs)
#[derive(Debug)]
pub struct SyncHttpResponse {
    pub status: u16,
    pub status_text: String,
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub timings: RequestTimings,
    pub proto: String,
    pub set_cookie_headers: Vec<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
}

impl<'js> IntoJs<'js> for SyncHttpResponse {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;

        // Set prototype for shared methods (json, bodyContains, etc.)
        // The prototype is created once per worker in register_sync_http()
        let globals = ctx.globals();
        if let Ok(proto) = globals.get::<_, Object>("__httpResponseProto") {
            let _ = obj.set_prototype(Some(&proto));
        }

        obj.set("status", self.status)?;
        obj.set("status_text", &self.status_text)?;
        // Keep both statusText and status_text for compatibility
        obj.set("statusText", &self.status_text)?;
        obj.set("proto", &self.proto)?;

        let body_str = String::from_utf8_lossy(&self.body);
        obj.set("body", body_str.as_ref())?;

        let headers_obj = Object::new(ctx.clone())?;
        for (k, v) in &self.headers {
            headers_obj.set(k.as_str(), v.as_str())?;
        }
        obj.set("headers", headers_obj)?;

        let timings_obj = Object::new(ctx.clone())?;
        timings_obj.set("duration", self.timings.duration.as_secs_f64() * 1000.0)?;
        timings_obj.set("blocked", 0.0)?;
        timings_obj.set("connecting", 0.0)?;
        timings_obj.set("tls_handshaking", 0.0)?;
        timings_obj.set("sending", 0.0)?;
        timings_obj.set("waiting", self.timings.waiting.as_secs_f64() * 1000.0)?;
        timings_obj.set("receiving", self.timings.receiving.as_secs_f64() * 1000.0)?;
        obj.set("timings", timings_obj)?;

        // Cookies - only create cookie objects if there are Set-Cookie headers
        if !self.set_cookie_headers.is_empty() {
            let cookies_obj = Object::new(ctx.clone())?;
            for set_cookie_str in &self.set_cookie_headers {
                if let Ok(cookie) = Cookie::parse(set_cookie_str.as_str()) {
                    let cookie_info = Object::new(ctx.clone())?;
                    cookie_info.set("value", cookie.value())?;

                    if let Some(domain) = cookie.domain() {
                        cookie_info.set("domain", domain)?;
                    }
                    if let Some(path) = cookie.path() {
                        cookie_info.set("path", path)?;
                    }
                    if let Some(cookie::Expiration::DateTime(dt)) = cookie.expires() {
                        cookie_info.set("expires", dt.unix_timestamp())?;
                    }
                    if let Some(max_age) = cookie.max_age() {
                        cookie_info.set("maxAge", max_age.whole_seconds())?;
                    }
                    cookie_info.set("httpOnly", cookie.http_only().unwrap_or(false))?;
                    cookie_info.set("secure", cookie.secure().unwrap_or(false))?;
                    if let Some(same_site) = cookie.same_site() {
                        let ss_str = match same_site {
                            SameSite::Strict => "Strict",
                            SameSite::Lax => "Lax",
                            SameSite::None => "None",
                        };
                        cookie_info.set("sameSite", ss_str)?;
                    }

                    cookies_obj.set(cookie.name(), cookie_info)?;
                }
            }
            obj.set("cookies", cookies_obj)?;
        } else {
            let cookies_obj = Object::new(ctx.clone())?;
            obj.set("cookies", cookies_obj)?;
        }

        // Add error and errorCode fields for failed requests
        if let Some(ref err) = self.error {
            obj.set("error", err.clone())?;
        }
        if let Some(ref code) = self.error_code {
            obj.set("errorCode", code.clone())?;
        }

        Ok(obj.into_value())
    }
}

// Thread-local ureq agent for connection pooling
thread_local! {
    static AGENT: ureq::Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build();
}

/// Build an http::Request from JS parameters for IoBridge routing
fn build_http_request(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
) -> std::result::Result<http::Request<String>, String> {
    let mut builder = http::Request::builder().method(method).uri(url);

    for (k, v) in headers {
        builder = builder.header(k.as_str(), v.as_str());
    }

    let body_str = body.unwrap_or("").to_string();
    builder.body(body_str).map_err(|e| e.to_string())
}

/// Convert Hyper Response + RequestTimings to SyncHttpResponse
fn hyper_response_to_sync(
    response: http::Response<Bytes>,
    timings: RequestTimings,
) -> SyncHttpResponse {
    let status = response.status().as_u16();
    let mut headers = HashMap::new();
    let mut set_cookie_headers = Vec::new();

    for (name, value) in response.headers() {
        let name_str = name.as_str().to_string();
        let value_str = value.to_str().unwrap_or("").to_string();
        if name_str.to_lowercase() == "set-cookie" {
            set_cookie_headers.push(value_str.clone());
        }
        headers.insert(name_str, value_str);
    }

    let body = response.into_body().to_vec();

    SyncHttpResponse {
        status,
        status_text: status_text_for_code(status),
        body,
        headers,
        timings,
        proto: "h2".to_string(),
        set_cookie_headers,
        error: None,
        error_code: None,
    }
}

/// Execute an HTTP request through IoBridge (no-pool mode with connection timing)
fn execute_via_bridge(
    bridge: &IoBridge,
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
    response_sink: bool,
) -> std::result::Result<SyncHttpResponse, String> {
    let req = build_http_request(method, url, body, headers)?;
    let (response, timings) = bridge.request(req, Some(Duration::from_secs(60)), response_sink)?;
    Ok(hyper_response_to_sync(response, timings))
}

/// Minimal raw HTTP/1.1 client using std::net::TcpStream (may-cooperative).
/// Eliminates ureq overhead: no per-request URL parsing, no middleware, no connection pool locking.
mod raw_http {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    /// Result of a raw HTTP request: (status_code, headers, body)
    type RawResult = Result<(u16, HashMap<String, String>, Vec<u8>), String>;

    pub struct ParsedUrl {
        pub host_port: String,
        pub path: String,
    }

    pub fn parse_url(url: &str) -> Option<ParsedUrl> {
        let without_scheme = url.strip_prefix("http://")?;
        let (host_port, path) = match without_scheme.find('/') {
            Some(i) => (&without_scheme[..i], &without_scheme[i..]),
            None => (without_scheme, "/"),
        };
        let host_port = if host_port.contains(':') {
            host_port.to_string()
        } else {
            format!("{}:80", host_port)
        };
        Some(ParsedUrl {
            host_port,
            path: path.to_string(),
        })
    }

    pub struct RawConn {
        stream: RefCell<Option<BufReader<TcpStream>>>,
        host_port: RefCell<String>,
    }

    impl RawConn {
        pub fn new() -> Self {
            RawConn {
                stream: RefCell::new(None),
                host_port: RefCell::new(String::new()),
            }
        }

        pub fn get(
            &self,
            parsed: &ParsedUrl,
            extra_headers: &HashMap<String, String>,
            discard_body: bool,
        ) -> RawResult {
            // Reconnect if host changed or connection dropped
            let need_connect = {
                let hp = self.host_port.borrow();
                let stream = self.stream.borrow();
                *hp != parsed.host_port || stream.is_none()
            };

            if need_connect {
                match TcpStream::connect(&parsed.host_port) {
                    Ok(tcp) => {
                        tcp.set_nodelay(true).ok();
                        *self.host_port.borrow_mut() = parsed.host_port.clone();
                        *self.stream.borrow_mut() = Some(BufReader::new(tcp));
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }

            // Try the request; on failure, reconnect once and retry
            match self.do_get(parsed, extra_headers, discard_body) {
                Ok(r) => Ok(r),
                Err(_) => {
                    // Reconnect and retry
                    match TcpStream::connect(&parsed.host_port) {
                        Ok(tcp) => {
                            tcp.set_nodelay(true).ok();
                            *self.stream.borrow_mut() = Some(BufReader::new(tcp));
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                    self.do_get(parsed, extra_headers, discard_body)
                }
            }
        }

        fn do_get(
            &self,
            parsed: &ParsedUrl,
            extra_headers: &HashMap<String, String>,
            discard_body: bool,
        ) -> RawResult {
            let mut stream_ref = self.stream.borrow_mut();
            let reader = stream_ref.as_mut().ok_or("no connection")?;

            // Write request
            let inner = reader.get_mut();
            let mut req = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n",
                parsed.path, parsed.host_port
            );
            for (k, v) in extra_headers {
                req.push_str(k);
                req.push_str(": ");
                req.push_str(v);
                req.push_str("\r\n");
            }
            req.push_str("\r\n");
            inner.write_all(req.as_bytes()).map_err(|e| e.to_string())?;

            // Read status line
            let mut line = String::new();
            reader.read_line(&mut line).map_err(|e| e.to_string())?;
            let status = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(0);

            // Read headers
            let mut headers = HashMap::new();
            let mut content_length: usize = 0;
            let mut chunked = false;
            loop {
                line.clear();
                reader.read_line(&mut line).map_err(|e| e.to_string())?;
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((k, v)) = line.trim_end().split_once(": ") {
                    let k_lower = k.to_lowercase();
                    if k_lower == "content-length" {
                        content_length = v.parse().unwrap_or(0);
                    } else if k_lower == "transfer-encoding" && v.contains("chunked") {
                        chunked = true;
                    }
                    headers.insert(k.to_string(), v.to_string());
                }
            }

            // Read body
            let body = if chunked {
                // Simple chunked reader
                let mut body = Vec::new();
                loop {
                    line.clear();
                    reader.read_line(&mut line).map_err(|e| e.to_string())?;
                    let chunk_size = usize::from_str_radix(line.trim(), 16).unwrap_or(0);
                    if chunk_size == 0 {
                        // Read trailing \r\n
                        line.clear();
                        let _ = reader.read_line(&mut line);
                        break;
                    }
                    let mut chunk = vec![0u8; chunk_size];
                    reader.read_exact(&mut chunk).map_err(|e| e.to_string())?;
                    if !discard_body {
                        body.extend_from_slice(&chunk);
                    }
                    // Read trailing \r\n
                    line.clear();
                    let _ = reader.read_line(&mut line);
                }
                body
            } else if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).map_err(|e| e.to_string())?;
                if discard_body {
                    Vec::new()
                } else {
                    body
                }
            } else {
                Vec::new()
            };

            Ok((status, headers, body))
        }
    }
}

/// Register synchronous HTTP functions using ureq (no Tokio overhead)
/// When io_bridge is Some, routes through Hyper with pool disabled for connection timing.
pub fn register_sync_http(
    ctx: &Ctx,
    tx: Sender<Metric>,
    response_sink: bool,
    io_bridge: Option<Arc<IoBridge>>,
) -> Result<()> {
    let http = Object::new(ctx.clone())?;
    let global_response_sink = response_sink;

    // Create response prototype once — shared by all responses from this worker.
    // Methods use `this.body` and `this.headers` so they work on any response instance.
    ctx.eval::<(), _>(r#"
        globalThis.__httpResponseProto = {
            json() { return JSON.parse(this.body); },
            bodyContains(str) { return this.body.includes(str); },
            bodyMatches(pattern) { return new RegExp(pattern).test(this.body); },
            hasHeader(name, value) {
                const h = this.headers;
                if (!h) return false;
                // Direct lookup
                if (h[name] !== undefined) {
                    return value === undefined ? true : h[name] === value;
                }
                // Case-insensitive lookup
                const lower = name.toLowerCase();
                for (const k in h) {
                    if (k.toLowerCase() === lower) {
                        return value === undefined ? true : h[k] === value;
                    }
                }
                return false;
            },
            isJson() {
                const ct = this.headers && (this.headers['content-type'] || this.headers['Content-Type']);
                return ct ? ct.includes('application/json') : false;
            }
        };
    "#)?;

    // GET - most common, highly optimized
    let tx_get = tx.clone();
    let sink_get = global_response_sink;
    let bridge_get = io_bridge.clone();
    let raw_conn = raw_http::RawConn::new();
    http.set(
        "get",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let custom_headers: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .unwrap_or_default();

                let tx = tx_get.clone();

                // IoBridge path: route through Hyper with no pool for connection timing
                if let Some(ref bridge) = bridge_get {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "GET",
                        &url_str,
                        None,
                        &custom_headers,
                        sink_get,
                    ) {
                        Ok(mut resp) => {
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            resp.timings.duration = start.elapsed();
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }

                let start = Instant::now();

                // Use raw TCP for http:// URLs (fast path), fall back to ureq for https://
                let parsed = raw_http::parse_url(&url_str);
                let use_raw = parsed.is_some() && url_str.starts_with("http://");

                if use_raw {
                    let parsed = parsed.unwrap();
                    match raw_conn.get(&parsed, &custom_headers, sink_get) {
                        Ok((status, headers, body)) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                waiting: duration,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };

                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status,
                                error: None,
                                tags,
                            });

                            let set_cookie_headers = headers
                                .iter()
                                .filter(|(k, _)| k.to_lowercase() == "set-cookie")
                                .map(|(_, v)| v.clone())
                                .collect();

                            Ok(SyncHttpResponse {
                                status,
                                status_text: status_text_for_code(status),
                                body,
                                headers,
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers,
                                error: None,
                                error_code: None,
                            })
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };

                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });

                            Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            })
                        }
                    }
                } else {
                    // Fallback to ureq for https:// and other schemes
                    let result = AGENT.with(|agent| {
                        let mut req = agent.get(&url_str);
                        for (k, v) in &custom_headers {
                            req = req.set(k, v);
                        }
                        req.call()
                    });

                    let waiting = start.elapsed();

                    match result {
                        Ok(response) => {
                            let status = response.status();
                            let mut headers = HashMap::new();
                            let mut set_cookie_headers = Vec::new();
                            for name in response.headers_names() {
                                if let Some(val) = response.header(&name) {
                                    if name.to_lowercase() == "set-cookie" {
                                        set_cookie_headers.push(val.to_string());
                                    }
                                    headers.insert(name, val.to_string());
                                }
                            }

                            let body_start = Instant::now();
                            let body = if sink_get {
                                let _ = response.into_string();
                                Vec::new()
                            } else {
                                response.into_string().unwrap_or_default().into_bytes()
                            };
                            let receiving = body_start.elapsed();
                            let duration = start.elapsed();

                            let timings = RequestTimings {
                                duration,
                                waiting,
                                receiving,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };

                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status,
                                error: None,
                                tags,
                            });

                            Ok(SyncHttpResponse {
                                status,
                                status_text: status_text_for_code(status),
                                body,
                                headers,
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers,
                                error: None,
                                error_code: None,
                            })
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };

                            let error_msg = e.to_string();
                            let (error_type, error_code) = categorize_error(&error_msg);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(error_msg.clone()),
                                tags,
                            });

                            Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: error_msg.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            })
                        }
                    }
                }
            },
        ),
    )?;

    let tx_post = tx.clone();
    let sink_post = global_response_sink;
    let bridge_post = io_bridge.clone();
    http.set(
        "post",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let content_type: String = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .and_then(|h| h.get("Content-Type").cloned())
                    .unwrap_or_else(|| "application/json".to_string());

                let tx = tx_post.clone();

                // IoBridge path
                if let Some(ref bridge) = bridge_post {
                    let mut hdrs = HashMap::new();
                    hdrs.insert("Content-Type".to_string(), content_type.clone());
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "POST",
                        &url_str,
                        Some(&body),
                        &hdrs,
                        sink_post,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }

                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    agent
                        .post(&url_str)
                        .set("Content-Type", &content_type)
                        .send_string(&body)
                });

                let waiting = start.elapsed();

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }

                        let body_start = Instant::now();
                        let resp_body = if sink_post {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };
                        let receiving = body_start.elapsed();
                        let duration = start.elapsed();

                        let timings = RequestTimings {
                            duration,
                            waiting,
                            receiving,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let duration = start.elapsed();
                        let timings = RequestTimings {
                            duration,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    let tx_put = tx.clone();
    let sink_put = global_response_sink;
    let bridge_put = io_bridge.clone();
    http.set(
        "put",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let tx = tx_put.clone();
                // IoBridge path
                if let Some(ref bridge) = bridge_put {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "PUT",
                        &url_str,
                        Some(&body),
                        &HashMap::new(),
                        sink_put,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    agent
                        .put(&url_str)
                        .set("Content-Type", "application/json")
                        .send_string(&body)
                });

                let duration = start.elapsed();
                let timings = RequestTimings {
                    duration,
                    ..Default::default()
                };

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }
                        let resp_body = if sink_put {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };
                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });
                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    let tx_del = tx.clone();
    let sink_del = global_response_sink;
    let bridge_del = io_bridge.clone();
    http.set(
        "del",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let custom_headers: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .unwrap_or_default();

                let tx = tx_del.clone();
                // IoBridge path
                if let Some(ref bridge) = bridge_del {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "DELETE",
                        &url_str,
                        None,
                        &custom_headers,
                        sink_del,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    let mut req = agent.delete(&url_str);
                    for (k, v) in &custom_headers {
                        req = req.set(k, v);
                    }
                    req.call()
                });

                let duration = start.elapsed();
                let timings = RequestTimings {
                    duration,
                    ..Default::default()
                };

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }
                        let resp_body = if sink_del {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };
                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });
                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    // PATCH - like POST, takes url + body + rest args
    let tx_patch = tx.clone();
    let sink_patch = global_response_sink;
    let bridge_patch = io_bridge.clone();
    http.set(
        "patch",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let content_type: String = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .and_then(|h| h.get("Content-Type").cloned())
                    .unwrap_or_else(|| "application/json".to_string());

                let tx = tx_patch.clone();
                // IoBridge path
                if let Some(ref bridge) = bridge_patch {
                    let mut hdrs = HashMap::new();
                    hdrs.insert("Content-Type".to_string(), content_type.clone());
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "PATCH",
                        &url_str,
                        Some(&body),
                        &hdrs,
                        sink_patch,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    agent
                        .request("PATCH", &url_str)
                        .set("Content-Type", &content_type)
                        .send_string(&body)
                });

                let waiting = start.elapsed();

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }

                        let body_start = Instant::now();
                        let resp_body = if sink_patch {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };
                        let receiving = body_start.elapsed();
                        let duration = start.elapsed();

                        let timings = RequestTimings {
                            duration,
                            waiting,
                            receiving,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let duration = start.elapsed();
                        let timings = RequestTimings {
                            duration,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    // HEAD - like GET but uses .head() and doesn't read body
    let tx_head = tx.clone();
    let sink_head = global_response_sink;
    let bridge_head = io_bridge.clone();
    http.set(
        "head",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let custom_headers: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .unwrap_or_default();

                let tx = tx_head.clone();
                // IoBridge path
                if let Some(ref bridge) = bridge_head {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "HEAD",
                        &url_str,
                        None,
                        &custom_headers,
                        sink_head,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    let mut req = agent.head(&url_str);
                    for (k, v) in &custom_headers {
                        req = req.set(k, v);
                    }
                    req.call()
                });

                let duration = start.elapsed();
                let timings = RequestTimings {
                    duration,
                    ..Default::default()
                };

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: Vec::new(),
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };
                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });
                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    // OPTIONS - like GET but uses agent.request("OPTIONS", ...)
    let tx_options = tx.clone();
    let sink_options = global_response_sink;
    let bridge_options = io_bridge.clone();
    http.set(
        "options",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let custom_headers: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .unwrap_or_default();

                let tx = tx_options.clone();
                // IoBridge path
                if let Some(ref bridge) = bridge_options {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        "OPTIONS",
                        &url_str,
                        None,
                        &custom_headers,
                        sink_options,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    let mut req = agent.request("OPTIONS", &url_str);
                    for (k, v) in &custom_headers {
                        req = req.set(k, v);
                    }
                    req.call()
                });

                let waiting = start.elapsed();

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }

                        let body_start = Instant::now();
                        let resp_body = if sink_options {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };
                        let receiving = body_start.elapsed();
                        let duration = start.elapsed();

                        let timings = RequestTimings {
                            duration,
                            waiting,
                            receiving,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let duration = start.elapsed();
                        let timings = RequestTimings {
                            duration,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    // REQUEST - generic HTTP method, takes method as first parameter
    let tx_request = tx.clone();
    let sink_request = global_response_sink;
    let bridge_request = io_bridge.clone();
    http.set(
        "request",
        Function::new(
            ctx.clone(),
            move |method_str: String,
                  url_str: String,
                  body: Option<String>,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let name_tag: Option<String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, String>("name").ok());

                let tags: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get("tags").ok())
                    .unwrap_or_default();

                let custom_headers: HashMap<String, String> = rest
                    .first()
                    .and_then(|arg| arg.as_object())
                    .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
                    .unwrap_or_default();

                let tx = tx_request.clone();
                let method_upper = method_str.to_uppercase();

                // IoBridge path
                if let Some(ref bridge) = bridge_request {
                    let start = Instant::now();
                    match execute_via_bridge(
                        bridge,
                        &method_upper,
                        &url_str,
                        body.as_deref(),
                        &custom_headers,
                        sink_request,
                    ) {
                        Ok(mut resp) => {
                            resp.timings.duration = start.elapsed();
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings: resp.timings,
                                status: resp.status,
                                error: None,
                                tags,
                            });
                            return Ok(resp);
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let metric_name: Cow<str> = match &name_tag {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url_str),
                            };
                            let (error_type, error_code) = categorize_error(&e);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.clone()),
                                tags,
                            });
                            return Ok(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: e.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                let start = Instant::now();

                let result = AGENT.with(|agent| {
                    let mut req = agent.request(&method_upper, &url_str);
                    for (k, v) in &custom_headers {
                        req = req.set(k, v);
                    }
                    if let Some(ref b) = body {
                        req.send_string(b)
                    } else {
                        req.call()
                    }
                });

                let waiting = start.elapsed();

                match result {
                    Ok(response) => {
                        let status = response.status();
                        let mut headers = HashMap::new();
                        let mut set_cookie_headers = Vec::new();
                        for name in response.headers_names() {
                            if let Some(val) = response.header(&name) {
                                if name.to_lowercase() == "set-cookie" {
                                    set_cookie_headers.push(val.to_string());
                                }
                                headers.insert(name, val.to_string());
                            }
                        }

                        let body_start = Instant::now();
                        let resp_body = if sink_request {
                            let _ = response.into_string();
                            Vec::new()
                        } else {
                            response.into_string().unwrap_or_default().into_bytes()
                        };
                        let receiving = body_start.elapsed();
                        let duration = start.elapsed();

                        let timings = RequestTimings {
                            duration,
                            waiting,
                            receiving,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body: resp_body,
                            headers,
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let duration = start.elapsed();
                        let timings = RequestTimings {
                            duration,
                            ..Default::default()
                        };

                        let metric_name: Cow<str> = match &name_tag {
                            Some(n) => Cow::Borrowed(n.as_str()),
                            None => Cow::Borrowed(&url_str),
                        };

                        let error_msg = e.to_string();
                        let (error_type, error_code) = categorize_error(&error_msg);
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(error_msg.clone()),
                            tags,
                        });

                        Ok(SyncHttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: error_msg.into_bytes(),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                            set_cookie_headers: Vec::new(),
                            error: Some(error_type),
                            error_code: Some(error_code),
                        })
                    }
                }
            },
        ),
    )?;

    // BATCH - execute an array of requests sequentially
    let tx_batch = tx.clone();
    let sink_batch = global_response_sink;
    let bridge_batch = io_bridge.clone();
    http.set(
        "batch",
        Function::new(
            ctx.clone(),
            move |requests: Array<'_>| -> Result<Vec<SyncHttpResponse>> {
                let mut results = Vec::new();
                for item in requests.iter::<Object>() {
                    let obj = item?;
                    let method: String = obj.get("method").unwrap_or_else(|_| "GET".to_string());
                    let url: String = obj.get("url")?;
                    let body: Option<String> = obj.get("body").ok();
                    let headers: HashMap<String, String> = obj.get("headers").unwrap_or_default();
                    let name: Option<String> = obj.get("name").ok();
                    let tags: HashMap<String, String> = obj.get("tags").unwrap_or_default();
                    let tx = tx_batch.clone();
                    let method_upper = method.to_uppercase();

                    // IoBridge path
                    if let Some(ref bridge) = bridge_batch {
                        let start = Instant::now();
                        match execute_via_bridge(
                            bridge,
                            &method_upper,
                            &url,
                            body.as_deref(),
                            &headers,
                            sink_batch,
                        ) {
                            Ok(mut resp) => {
                                resp.timings.duration = start.elapsed();
                                let metric_name: Cow<str> = match &name {
                                    Some(n) => Cow::Borrowed(n.as_str()),
                                    None => Cow::Borrowed(&url),
                                };
                                let _ = tx.send(Metric::Request {
                                    name: format!(
                                        "{}{}",
                                        crate::bridge::group::get_current_group_prefix(),
                                        metric_name
                                    ),
                                    timings: resp.timings,
                                    status: resp.status,
                                    error: None,
                                    tags,
                                });
                                results.push(resp);
                                continue;
                            }
                            Err(e) => {
                                let duration = start.elapsed();
                                let timings = RequestTimings {
                                    duration,
                                    ..Default::default()
                                };
                                let metric_name: Cow<str> = match &name {
                                    Some(n) => Cow::Borrowed(n.as_str()),
                                    None => Cow::Borrowed(&url),
                                };
                                let (error_type, error_code) = categorize_error(&e);
                                let _ = tx.send(Metric::Request {
                                    name: format!(
                                        "{}{}",
                                        crate::bridge::group::get_current_group_prefix(),
                                        metric_name
                                    ),
                                    timings,
                                    status: 0,
                                    error: Some(e.clone()),
                                    tags,
                                });
                                results.push(SyncHttpResponse {
                                    status: 0,
                                    status_text: status_text_for_code(0),
                                    body: e.into_bytes(),
                                    headers: HashMap::new(),
                                    timings,
                                    proto: "h1".to_string(),
                                    set_cookie_headers: Vec::new(),
                                    error: Some(error_type),
                                    error_code: Some(error_code),
                                });
                                continue;
                            }
                        }
                    }

                    // ureq path
                    let start = Instant::now();
                    let result = AGENT.with(|agent| {
                        let mut req = agent.request(&method_upper, &url);
                        for (k, v) in &headers {
                            req = req.set(k, v);
                        }
                        if let Some(ref b) = body {
                            req.send_string(b)
                        } else {
                            req.call()
                        }
                    });
                    let waiting = start.elapsed();

                    match result {
                        Ok(response) => {
                            let status = response.status();
                            let mut resp_headers = HashMap::new();
                            let mut set_cookie_headers = Vec::new();
                            for hdr_name in response.headers_names() {
                                if let Some(val) = response.header(&hdr_name) {
                                    if hdr_name.to_lowercase() == "set-cookie" {
                                        set_cookie_headers.push(val.to_string());
                                    }
                                    resp_headers.insert(hdr_name, val.to_string());
                                }
                            }

                            let body_start = Instant::now();
                            let resp_body = if sink_batch {
                                let _ = response.into_string();
                                Vec::new()
                            } else {
                                response.into_string().unwrap_or_default().into_bytes()
                            };
                            let receiving = body_start.elapsed();
                            let duration = start.elapsed();

                            let timings = RequestTimings {
                                duration,
                                waiting,
                                receiving,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url),
                            };

                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status,
                                error: None,
                                tags,
                            });

                            results.push(SyncHttpResponse {
                                status,
                                status_text: status_text_for_code(status),
                                body: resp_body,
                                headers: resp_headers,
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers,
                                error: None,
                                error_code: None,
                            });
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };

                            let metric_name: Cow<str> = match &name {
                                Some(n) => Cow::Borrowed(n.as_str()),
                                None => Cow::Borrowed(&url),
                            };

                            let error_msg = e.to_string();
                            let (error_type, error_code) = categorize_error(&error_msg);
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(error_msg.clone()),
                                tags,
                            });

                            results.push(SyncHttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: error_msg.into_bytes(),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some(error_type),
                                error_code: Some(error_code),
                            });
                        }
                    }
                }
                Ok(results)
            },
        ),
    )?;

    ctx.globals().set("http", http)?;

    // Initialize HTTP hooks infrastructure
    ctx.eval::<(), _>(
        r#"
        globalThis.__http_hooks = globalThis.__http_hooks || {
            beforeRequest: [],
            afterResponse: []
        };
        globalThis.__http_callBeforeRequestHooks = function(req) {
            for (var i = 0; i < globalThis.__http_hooks.beforeRequest.length; i++) {
                try { globalThis.__http_hooks.beforeRequest[i](req); } catch(e) {}
            }
        };
        globalThis.__http_callAfterResponseHooks = function(res) {
            for (var i = 0; i < globalThis.__http_hooks.afterResponse.length; i++) {
                try { globalThis.__http_hooks.afterResponse[i](res); } catch(e) {}
            }
        };
    "#,
    )?;

    ctx.eval::<(), _>(
        r#"
        globalThis.FormData = function() {
            this._fields = [];
            this._boundary = '----FusilladeBoundary' + Math.random().toString(16).substring(2);
        };
        FormData.prototype.append = function(name, value) {
            this._fields.push({ name: name, value: String(value), isFile: false });
        };
        FormData.prototype.body = function() {
            var body = '';
            for (var i = 0; i < this._fields.length; i++) {
                var field = this._fields[i];
                body += '--' + this._boundary + '\r\n';
                body += 'Content-Disposition: form-data; name="' + field.name + '"\r\n\r\n';
                body += field.value;
                body += '\r\n';
            }
            body += '--' + this._boundary + '--\r\n';
            return body;
        };
        FormData.prototype.contentType = function() {
            return 'multipart/form-data; boundary=' + this._boundary;
        };

        http.addHook = function(hookType, fn) {
            if (hookType === 'beforeRequest') {
                globalThis.__http_hooks.beforeRequest.push(fn);
            } else if (hookType === 'afterResponse') {
                globalThis.__http_hooks.afterResponse.push(fn);
            }
        };
        http.clearHooks = function() {
            globalThis.__http_hooks.beforeRequest = [];
            globalThis.__http_hooks.afterResponse = [];
        };
        http.graphql = function(url, query, variables, options) {
            var body = JSON.stringify({ query: query, variables: variables || null });
            var opts = options || {};
            opts.headers = opts.headers || {};
            opts.headers['Content-Type'] = 'application/json';
            return http.post(url, body, opts);
        };
    "#,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::raw_http::parse_url;

    #[test]
    fn parse_url_ip_no_port() {
        let parsed = parse_url("http://70.34.195.10/api/search/").unwrap();
        assert_eq!(parsed.host_port, "70.34.195.10:80");
        assert_eq!(parsed.path, "/api/search/");
    }

    #[test]
    fn parse_url_ip_with_port() {
        let parsed = parse_url("http://192.168.1.1:8080/path").unwrap();
        assert_eq!(parsed.host_port, "192.168.1.1:8080");
        assert_eq!(parsed.path, "/path");
    }

    #[test]
    fn parse_url_hostname_no_port() {
        let parsed = parse_url("http://example.com/api/test").unwrap();
        assert_eq!(parsed.host_port, "example.com:80");
        assert_eq!(parsed.path, "/api/test");
    }

    #[test]
    fn parse_url_hostname_with_port() {
        let parsed = parse_url("http://example.com:3000/api").unwrap();
        assert_eq!(parsed.host_port, "example.com:3000");
        assert_eq!(parsed.path, "/api");
    }

    #[test]
    fn parse_url_no_path() {
        let parsed = parse_url("http://example.com").unwrap();
        assert_eq!(parsed.host_port, "example.com:80");
        assert_eq!(parsed.path, "/");
    }

    #[test]
    fn parse_url_ip_no_path() {
        let parsed = parse_url("http://10.0.0.1").unwrap();
        assert_eq!(parsed.host_port, "10.0.0.1:80");
        assert_eq!(parsed.path, "/");
    }

    #[test]
    fn parse_url_rejects_https() {
        assert!(parse_url("https://example.com/api").is_none());
    }

    #[test]
    fn parse_url_rejects_garbage() {
        assert!(parse_url("not-a-url").is_none());
    }
}
