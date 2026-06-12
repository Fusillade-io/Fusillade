//! Synchronous HTTP bridge using ureq - bypasses Tokio for lower latency
//! This module provides a drop-in replacement for http.rs that uses blocking I/O
//! directly compatible with May green threads.
//!
// ureq::Error is large by design; suppressing here since we can't change the external type.
#![allow(clippy::result_large_err)]
//! When an IoBridge is provided (no_pool mode), requests are routed through
//! Hyper with connection pooling disabled, providing per-request DNS+TCP and TLS timing.

use crate::engine::io_bridge::IoBridge;
use crate::stats::{Metric, RequestTimings};
use cookie::{Cookie, SameSite};
use crossbeam_channel::Sender;
use hyper::body::Bytes;
use rquickjs::{Array, Ctx, Function, IntoJs, Object, Result, Value};
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

// Thread-local ureq agent for connection pooling. Auto-redirects are
// disabled: redirects are followed manually in execute_ureq_request so that
// per-hop Set-Cookie headers are captured and max_redirects is honored.
thread_local! {
    static AGENT: ureq::Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .redirects(0)
        .build();
}

/// Process-wide default for the maximum number of redirects to follow,
/// set from config (`max_redirects`) at engine startup. Per-request
/// `maxRedirects` / `followRedirects: false` options override it.
static DEFAULT_MAX_REDIRECTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(10);

pub fn set_default_max_redirects(n: u32) {
    DEFAULT_MAX_REDIRECTS.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// Resolve the redirect limit for one request from its options object.
fn max_redirects_from_options(options: Option<&Value<'_>>) -> u32 {
    if let Some(obj) = options.and_then(|v| v.as_object()) {
        if let Ok(false) = obj.get::<_, bool>("followRedirects") {
            return 0;
        }
        if let Ok(n) = obj.get::<_, u32>("maxRedirects") {
            return n;
        }
    }
    DEFAULT_MAX_REDIRECTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// If `status` redirects to `location`, return the absolute next URL and
/// whether the method must collapse to GET (301/302/303; 307/308 keep the
/// method and body).
fn redirect_target(
    status: u16,
    location: Option<&str>,
    current_url: &str,
) -> Option<(String, bool)> {
    if !matches!(status, 301..=303 | 307 | 308) {
        return None;
    }
    let next = url::Url::parse(current_url).ok()?.join(location?).ok()?;
    Some((next.to_string(), matches!(status, 301..=303)))
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

fn execute_via_bridge(
    bridge: &IoBridge,
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
    timeout: Option<Duration>,
    response_sink: bool,
) -> std::result::Result<SyncHttpResponse, String> {
    let req = build_http_request(method, url, body, headers)?;
    let (response, timings) = bridge.request(
        req,
        Some(timeout.unwrap_or(Duration::from_secs(60))),
        response_sink,
    )?;
    Ok(hyper_response_to_sync(response, timings))
}

/// Extract the per-request `timeout` option (e.g. '500ms', '10s') from the JS
/// options object. Numbers are treated as milliseconds.
fn timeout_from_options(options: Option<&Value<'_>>) -> Option<Duration> {
    let obj = options?.as_object()?;
    if let Ok(raw) = obj.get::<_, String>("timeout") {
        return crate::utils::parse_duration_str(&raw);
    }
    if let Ok(ms) = obj.get::<_, f64>("timeout") {
        if ms > 0.0 {
            return Some(Duration::from_millis(ms as u64));
        }
    }
    None
}

/// Approximate on-the-wire request size: request line + headers + body
/// (mirrors HttpClient::request's estimate for the pooled Hyper path).
fn estimate_request_size(
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body_len: usize,
) -> usize {
    let mut size = body_len;
    size += method.len() + 1 + url.len() + 11;
    for (k, v) in headers {
        size += k.len() + 2 + v.len() + 2;
    }
    size + 2
}

/// Build the metric name for a request: group prefix + user `name` tag or URL.
fn metric_name(name_tag: Option<&str>, url: &str) -> String {
    format!(
        "{}{}",
        crate::bridge::group::get_current_group_prefix(),
        name_tag.unwrap_or(url)
    )
}

/// Execute a request through the IoBridge (no_pool mode), following
/// redirects manually (capturing per-hop cookies), emit the request metric,
/// and convert the outcome into a SyncHttpResponse.
#[allow(clippy::too_many_arguments)]
fn run_bridge_request(
    bridge: &IoBridge,
    method: &str,
    url_str: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
    timeout: Option<Duration>,
    max_redirects: u32,
    response_sink: bool,
    tx: &Sender<Metric>,
    name_tag: Option<&str>,
    tags: HashMap<String, String>,
) -> SyncHttpResponse {
    let mut method = method.to_string();
    let mut current_url = url_str.to_string();
    let mut headers = headers.clone();
    let mut body: Option<String> = body.map(str::to_string);
    let mut hops = 0u32;

    let start = Instant::now();
    loop {
        let mut hop_headers = headers.clone();
        apply_jar_cookies(&current_url, &mut hop_headers);

        match execute_via_bridge(
            bridge,
            &method,
            &current_url,
            body.as_deref(),
            &hop_headers,
            timeout,
            response_sink,
        ) {
            Ok(mut resp) => {
                store_response_cookies(&current_url, &resp.set_cookie_headers);

                if hops < max_redirects {
                    let location = resp.headers.iter().find_map(|(k, v)| {
                        k.eq_ignore_ascii_case("location").then_some(v.as_str())
                    });
                    if let Some((next_url, collapse_to_get)) =
                        redirect_target(resp.status, location, &current_url)
                    {
                        if collapse_to_get && method != "GET" && method != "HEAD" {
                            method = "GET".to_string();
                            body = None;
                            headers.retain(|k, _| {
                                !k.eq_ignore_ascii_case("content-type")
                                    && !k.eq_ignore_ascii_case("content-length")
                            });
                        }
                        current_url = next_url;
                        hops += 1;
                        continue;
                    }
                }

                resp.timings.duration = start.elapsed();
                let _ = tx.send(Metric::Request {
                    name: metric_name(name_tag, url_str),
                    timings: resp.timings,
                    status: resp.status,
                    error: None,
                    tags,
                });
                return resp;
            }
            Err(e) => {
                let timings = RequestTimings {
                    duration: start.elapsed(),
                    ..Default::default()
                };
                let (error_type, error_code) = categorize_error(&e);
                let _ = tx.send(Metric::Request {
                    name: metric_name(name_tag, url_str),
                    timings,
                    status: 0,
                    error: Some(e.clone()),
                    tags,
                });
                return SyncHttpResponse {
                    status: 0,
                    status_text: status_text_for_code(0),
                    body: e.into_bytes(),
                    headers: HashMap::new(),
                    timings,
                    proto: "h1".to_string(),
                    set_cookie_headers: Vec::new(),
                    error: Some(error_type),
                    error_code: Some(error_code),
                };
            }
        }
    }
}

/// Execute a request over the pooled ureq agent, following redirects manually
/// up to `max_redirects` hops (capturing per-hop Set-Cookie headers), emit
/// the request metric, and convert the outcome into a SyncHttpResponse.
///
/// ureq reports non-2xx responses as `Err(Error::Status)`; those are real HTTP
/// responses (the JS `status` field is documented as the HTTP status code,
/// with 0 reserved for network/timeout errors), so they keep their status,
/// headers, and body. Only transport-level failures map to status 0. When the
/// redirect limit is exhausted, the last 3xx response is returned as-is.
#[allow(clippy::too_many_arguments)]
fn execute_ureq_request(
    method: &str,
    url_str: &str,
    headers: &HashMap<String, String>,
    body: Option<&str>,
    timeout: Option<Duration>,
    max_redirects: u32,
    response_sink: bool,
    tx: &Sender<Metric>,
    name_tag: Option<String>,
    tags: HashMap<String, String>,
) -> SyncHttpResponse {
    let request_size = estimate_request_size(method, url_str, headers, body.map_or(0, str::len));

    let mut method = method.to_string();
    let mut current_url = url_str.to_string();
    let mut headers = headers.clone();
    let mut body: Option<String> = body.map(str::to_string);
    let mut hops = 0u32;

    let start = Instant::now();
    loop {
        let mut hop_headers = headers.clone();
        apply_jar_cookies(&current_url, &mut hop_headers);

        let req = AGENT.with(|agent| {
            let mut req = agent.request(&method, &current_url);
            for (k, v) in &hop_headers {
                req = req.set(k, v);
            }
            match timeout {
                Some(t) => req.timeout(t),
                None => req,
            }
        });

        let hop_start = Instant::now();
        let result = match &body {
            Some(b) => req.send_string(b),
            None => req.call(),
        };
        let waiting = hop_start.elapsed();

        // Non-2xx is a real HTTP response, not a transport failure.
        let result = match result {
            Err(ureq::Error::Status(_, response)) => Ok(response),
            other => other,
        };

        match result {
            Ok(response) => {
                let status = response.status();
                let mut resp_headers = HashMap::new();
                let mut set_cookie_headers = Vec::new();
                for name in response.headers_names() {
                    if let Some(val) = response.header(&name) {
                        if name.to_lowercase() == "set-cookie" {
                            set_cookie_headers.push(val.to_string());
                        }
                        resp_headers.insert(name, val.to_string());
                    }
                }

                store_response_cookies(&current_url, &set_cookie_headers);

                if hops < max_redirects {
                    let location = resp_headers.iter().find_map(|(k, v)| {
                        k.eq_ignore_ascii_case("location").then_some(v.as_str())
                    });
                    if let Some((next_url, collapse_to_get)) =
                        redirect_target(status, location, &current_url)
                    {
                        // Drain the redirect body so the connection stays reusable.
                        let _ = response.into_string();
                        if collapse_to_get && method != "GET" && method != "HEAD" {
                            method = "GET".to_string();
                            body = None;
                            headers.retain(|k, _| {
                                !k.eq_ignore_ascii_case("content-type")
                                    && !k.eq_ignore_ascii_case("content-length")
                            });
                        }
                        current_url = next_url;
                        hops += 1;
                        continue;
                    }
                }

                let body_start = Instant::now();
                // Read the body even in sink mode so received bytes are tracked
                // and the connection stays reusable.
                let resp_body_str = response.into_string().unwrap_or_default();
                let body_len = resp_body_str.len();
                let resp_body = if response_sink {
                    Vec::new()
                } else {
                    resp_body_str.into_bytes()
                };
                let receiving = body_start.elapsed();
                let duration = start.elapsed();

                // Approximate wire size: status line + headers + body.
                let mut response_size = body_len + 15;
                for (k, v) in &resp_headers {
                    response_size += k.len() + 2 + v.len() + 2;
                }
                response_size += 2;

                let timings = RequestTimings {
                    duration,
                    waiting,
                    receiving,
                    request_size,
                    response_size,
                    pool_reused: pool_reuse_heuristic(&current_url),
                    ..Default::default()
                };

                let _ = tx.send(Metric::Request {
                    name: metric_name(name_tag.as_deref(), url_str),
                    timings,
                    status,
                    error: None,
                    tags,
                });

                return SyncHttpResponse {
                    status,
                    status_text: status_text_for_code(status),
                    body: resp_body,
                    headers: resp_headers,
                    timings,
                    proto: "h1".to_string(),
                    set_cookie_headers,
                    error: None,
                    error_code: None,
                };
            }
            Err(e) => {
                let duration = start.elapsed();
                let timings = RequestTimings {
                    duration,
                    request_size,
                    ..Default::default()
                };

                let error_msg = e.to_string();
                let (error_type, error_code) = categorize_error(&error_msg);
                let _ = tx.send(Metric::Request {
                    name: metric_name(name_tag.as_deref(), url_str),
                    timings,
                    status: 0,
                    error: Some(error_msg.clone()),
                    tags,
                });

                return SyncHttpResponse {
                    status: 0,
                    status_text: status_text_for_code(0),
                    body: error_msg.into_bytes(),
                    headers: HashMap::new(),
                    timings,
                    proto: "h1".to_string(),
                    set_cookie_headers: Vec::new(),
                    error: Some(error_type),
                    error_code: Some(error_code),
                };
            }
        }
    }
}

/// Extract the user `name` tag from the JS options object.
fn name_from_options(options: Option<&Value<'_>>) -> Option<String> {
    options?.as_object()?.get::<_, String>("name").ok()
}

/// Extract metric tags from the JS options object.
fn tags_from_options(options: Option<&Value<'_>>) -> HashMap<String, String> {
    options
        .and_then(|arg| arg.as_object())
        .and_then(|obj| obj.get("tags").ok())
        .unwrap_or_default()
}

/// Extract custom headers from the JS options object.
fn headers_from_options(options: Option<&Value<'_>>) -> HashMap<String, String> {
    options
        .and_then(|arg| arg.as_object())
        .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
        .unwrap_or_default()
}

/// Default the Content-Type header for body-carrying methods when the user
/// didn't set one (in any casing).
fn default_content_type(headers: &mut HashMap<String, String>) {
    if !headers
        .keys()
        .any(|k| k.eq_ignore_ascii_case("content-type"))
    {
        headers.insert("Content-Type".to_string(), "application/json".to_string());
    }
}

// Per-worker cookie jar: Set-Cookie responses are stored here and matching
// cookies are injected into every outgoing request (both the pooled ureq path
// and the IoBridge path). Also exposed to JS via http.cookieJar().
thread_local! {
    static COOKIE_JAR: std::cell::RefCell<cookie_store::CookieStore> =
        std::cell::RefCell::new(cookie_store::CookieStore::default());
}

/// Merge the jar's cookies for `url` into the outgoing headers. A user-set
/// Cookie header is kept; jar cookies are appended after it.
fn apply_jar_cookies(url: &str, headers: &mut HashMap<String, String>) {
    let Ok(parsed) = url::Url::parse(url) else {
        return;
    };
    let jar_cookies = COOKIE_JAR.with(|jar| {
        let jar = jar.borrow();
        let matched = jar.matches(&parsed);
        if matched.is_empty() {
            None
        } else {
            Some(
                matched
                    .iter()
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect::<Vec<_>>()
                    .join("; "),
            )
        }
    });
    let Some(jar_cookies) = jar_cookies else {
        return;
    };
    if let Some(existing) = headers
        .iter_mut()
        .find_map(|(k, v)| k.eq_ignore_ascii_case("cookie").then_some(v))
    {
        existing.push_str("; ");
        existing.push_str(&jar_cookies);
    } else {
        headers.insert("Cookie".to_string(), jar_cookies);
    }
}

/// Store response Set-Cookie headers into the worker's jar.
fn store_response_cookies(url: &str, set_cookie_headers: &[String]) {
    if set_cookie_headers.is_empty() {
        return;
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return;
    };
    COOKIE_JAR.with(|jar| {
        jar.borrow_mut().store_response_cookies(
            set_cookie_headers
                .iter()
                .filter_map(|h| Cookie::parse(h.clone()).ok()),
            &parsed,
        );
    });
}

fn parse_jar_url(url: &str) -> Result<url::Url> {
    url::Url::parse(url).map_err(|_| rquickjs::Error::new_from_js("invalid URL", "ValueError"))
}

fn stored_cookie_domain(cookie: &cookie_store::Cookie<'_>, fallback: &str) -> String {
    match &cookie.domain {
        cookie_store::CookieDomain::HostOnly(h) => h.clone(),
        cookie_store::CookieDomain::Suffix(s) => s.clone(),
        _ => fallback.to_string(),
    }
}

fn stored_cookie_to_js<'js>(
    ctx: &Ctx<'js>,
    cookie: &cookie_store::Cookie<'_>,
) -> Result<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    obj.set("name", cookie.name())?;
    obj.set("value", cookie.value())?;
    obj.set("domain", stored_cookie_domain(cookie, ""))?;
    obj.set("path", &*cookie.path)?;
    obj.set("secure", cookie.secure().unwrap_or(false))?;
    obj.set("httpOnly", cookie.http_only().unwrap_or(false))?;
    if let Some(cookie::Expiration::DateTime(dt)) = cookie.expires() {
        obj.set("expires", dt.unix_timestamp())?;
    }
    if let Some(max_age) = cookie.max_age() {
        obj.set("maxAge", max_age.whole_seconds())?;
    }
    if let Some(same_site) = cookie.same_site() {
        let ss = match same_site {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        };
        obj.set("sameSite", ss)?;
    }
    Ok(obj)
}

fn jar_set<'js>(
    _ctx: Ctx<'js>,
    url: String,
    name: String,
    value: String,
    opts: rquickjs::function::Opt<Object<'js>>,
) -> Result<()> {
    let parsed = parse_jar_url(&url)?;
    let mut cookie = Cookie::new(name, value);
    if let Some(o) = opts.0 {
        if let Ok(d) = o.get::<_, String>("domain") {
            cookie.set_domain(d);
        }
        if let Ok(p) = o.get::<_, String>("path") {
            cookie.set_path(p);
        }
        if let Ok(s) = o.get::<_, bool>("secure") {
            cookie.set_secure(s);
        }
        if let Ok(h) = o.get::<_, bool>("httpOnly") {
            cookie.set_http_only(h);
        }
        if let Ok(m) = o.get::<_, i64>("maxAge") {
            cookie.set_max_age(cookie::time::Duration::seconds(m));
        }
    }
    if cookie.path().is_none() {
        cookie.set_path("/");
    }
    COOKIE_JAR.with(|jar| {
        jar.borrow_mut()
            .insert_raw(&cookie, &parsed)
            .map(|_| ())
            .map_err(|_| rquickjs::Error::new_from_js("cookie rejected for URL", "ValueError"))
    })
}

fn jar_get<'js>(ctx: Ctx<'js>, url: String, name: String) -> Result<Value<'js>> {
    let parsed = parse_jar_url(&url)?;
    COOKIE_JAR.with(|jar| {
        let jar = jar.borrow();
        match jar.matches(&parsed).into_iter().find(|c| c.name() == name) {
            Some(cookie) => Ok(stored_cookie_to_js(&ctx, cookie)?.into_value()),
            None => Ok(Value::new_null(ctx.clone())),
        }
    })
}

fn jar_cookies_for_url<'js>(ctx: Ctx<'js>, url: String) -> Result<Vec<Object<'js>>> {
    let parsed = parse_jar_url(&url)?;
    COOKIE_JAR.with(|jar| {
        let jar = jar.borrow();
        jar.matches(&parsed)
            .into_iter()
            .map(|cookie| stored_cookie_to_js(&ctx, cookie))
            .collect()
    })
}

fn jar_delete<'js>(_ctx: Ctx<'js>, url: String, name: String) -> Result<()> {
    let parsed = parse_jar_url(&url)?;
    let host = parsed.host_str().unwrap_or("").to_string();
    COOKIE_JAR.with(|jar| {
        let mut jar = jar.borrow_mut();
        let targets: Vec<(String, String)> = jar
            .matches(&parsed)
            .into_iter()
            .filter(|c| c.name() == name)
            .map(|c| (stored_cookie_domain(c, &host), c.path.to_string()))
            .collect();
        for (domain, path) in targets {
            jar.remove(&domain, &path, &name);
        }
    });
    Ok(())
}

fn jar_clear() {
    COOKIE_JAR.with(|jar| jar.borrow_mut().clear());
}

/// Parse `html` and return the inner text of the first element matching the
/// CSS selector (empty string when nothing matches or the selector is
/// invalid). Backs the documented `res.html(selector)` helper.
fn html_select_first_text(html: &str, selector: &str) -> String {
    let document = scraper::Html::parse_document(html);
    let Ok(sel) = scraper::Selector::parse(selector) else {
        return String::new();
    };
    document
        .select(&sel)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
}

// ureq doesn't expose whether a pooled connection was reused, so derive a
// per-thread heuristic: the first successful request to a host:port is a
// pool miss, subsequent ones count as hits (the thread-local agent keeps
// the connection alive between requests).
thread_local! {
    static SEEN_HOSTS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

fn pool_reuse_heuristic(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let key = format!(
        "{}:{}",
        parsed.host_str().unwrap_or(""),
        parsed.port_or_known_default().unwrap_or(0)
    );
    SEEN_HOSTS.with(|seen| !seen.borrow_mut().insert(key))
}

/// URL parser for HTTP URLs - used in test utilities.
#[cfg(test)]
mod raw_http {
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

    // Native HTML query helper backing the response html() method
    ctx.globals().set(
        "__html_select_first_text",
        Function::new(ctx.clone(), |html: String, selector: String| -> String {
            html_select_first_text(&html, &selector)
        }),
    )?;

    // Create response prototype once — shared by all responses from this worker.
    // Methods use `this.body` and `this.headers` so they work on any response instance.
    ctx.eval::<(), _>(r#"
        globalThis.__httpResponseProto = {
            json() { return JSON.parse(this.body); },
            bodyContains(str) { return this.body.includes(str); },
            bodyMatches(pattern) { return new RegExp(pattern).test(this.body); },
            html(selector) { return globalThis.__html_select_first_text(this.body, selector); },
            matchesSchema(schema) {
                var data;
                try { data = JSON.parse(this.body); } catch (e) { return false; }
                if (typeof data !== 'object' || data === null) return false;
                for (var key in schema) {
                    var expected = schema[key];
                    var val = data[key];
                    if (val === undefined) return false;
                    if (expected === 'array') {
                        if (!Array.isArray(val)) return false;
                    } else if (expected === 'object') {
                        if (typeof val !== 'object' || val === null || Array.isArray(val)) return false;
                    } else if (typeof val !== expected) {
                        return false;
                    }
                }
                return true;
            },
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
    http.set(
        "get",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<SyncHttpResponse> {
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let custom_headers = headers_from_options(options);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_get.clone();

                if let Some(ref bridge) = bridge_get {
                    return Ok(run_bridge_request(
                        bridge,
                        "GET",
                        &url_str,
                        None,
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_get,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "GET",
                    &url_str,
                    &custom_headers,
                    None,
                    timeout,
                    max_redirects,
                    sink_get,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let mut custom_headers = headers_from_options(options);
                default_content_type(&mut custom_headers);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_post.clone();

                if let Some(ref bridge) = bridge_post {
                    return Ok(run_bridge_request(
                        bridge,
                        "POST",
                        &url_str,
                        Some(&body),
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_post,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "POST",
                    &url_str,
                    &custom_headers,
                    Some(&body),
                    timeout,
                    max_redirects,
                    sink_post,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let mut custom_headers = headers_from_options(options);
                default_content_type(&mut custom_headers);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_put.clone();

                if let Some(ref bridge) = bridge_put {
                    return Ok(run_bridge_request(
                        bridge,
                        "PUT",
                        &url_str,
                        Some(&body),
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_put,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "PUT",
                    &url_str,
                    &custom_headers,
                    Some(&body),
                    timeout,
                    max_redirects,
                    sink_put,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let custom_headers = headers_from_options(options);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_del.clone();

                if let Some(ref bridge) = bridge_del {
                    return Ok(run_bridge_request(
                        bridge,
                        "DELETE",
                        &url_str,
                        None,
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_del,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "DELETE",
                    &url_str,
                    &custom_headers,
                    None,
                    timeout,
                    max_redirects,
                    sink_del,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let mut custom_headers = headers_from_options(options);
                default_content_type(&mut custom_headers);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_patch.clone();

                if let Some(ref bridge) = bridge_patch {
                    return Ok(run_bridge_request(
                        bridge,
                        "PATCH",
                        &url_str,
                        Some(&body),
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_patch,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "PATCH",
                    &url_str,
                    &custom_headers,
                    Some(&body),
                    timeout,
                    max_redirects,
                    sink_patch,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let custom_headers = headers_from_options(options);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_head.clone();

                if let Some(ref bridge) = bridge_head {
                    return Ok(run_bridge_request(
                        bridge,
                        "HEAD",
                        &url_str,
                        None,
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_head,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "HEAD",
                    &url_str,
                    &custom_headers,
                    None,
                    timeout,
                    max_redirects,
                    sink_head,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let custom_headers = headers_from_options(options);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_options.clone();

                if let Some(ref bridge) = bridge_options {
                    return Ok(run_bridge_request(
                        bridge,
                        "OPTIONS",
                        &url_str,
                        None,
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_options,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    "OPTIONS",
                    &url_str,
                    &custom_headers,
                    None,
                    timeout,
                    max_redirects,
                    sink_options,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                let options = rest.first();
                let name_tag = name_from_options(options);
                let tags = tags_from_options(options);
                let custom_headers = headers_from_options(options);
                let timeout = timeout_from_options(options);
                let max_redirects = max_redirects_from_options(options);
                let tx = tx_request.clone();
                let method_upper = method_str.to_uppercase();

                if let Some(ref bridge) = bridge_request {
                    return Ok(run_bridge_request(
                        bridge,
                        &method_upper,
                        &url_str,
                        body.as_deref(),
                        &custom_headers,
                        timeout,
                        max_redirects,
                        sink_request,
                        &tx,
                        name_tag.as_deref(),
                        tags,
                    ));
                }

                Ok(execute_ureq_request(
                    &method_upper,
                    &url_str,
                    &custom_headers,
                    body.as_deref(),
                    timeout,
                    max_redirects,
                    sink_request,
                    &tx,
                    name_tag,
                    tags,
                ))
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
                    let timeout: Option<Duration> = obj
                        .get::<_, String>("timeout")
                        .ok()
                        .and_then(|s| crate::utils::parse_duration_str(&s));
                    let max_redirects: u32 =
                        if obj.get::<_, bool>("followRedirects").ok() == Some(false) {
                            0
                        } else {
                            obj.get::<_, u32>("maxRedirects").unwrap_or_else(|_| {
                                DEFAULT_MAX_REDIRECTS.load(std::sync::atomic::Ordering::Relaxed)
                            })
                        };
                    let tx = tx_batch.clone();
                    let method_upper = method.to_uppercase();

                    if let Some(ref bridge) = bridge_batch {
                        results.push(run_bridge_request(
                            bridge,
                            &method_upper,
                            &url,
                            body.as_deref(),
                            &headers,
                            timeout,
                            max_redirects,
                            sink_batch,
                            &tx,
                            name.as_deref(),
                            tags,
                        ));
                        continue;
                    }

                    results.push(execute_ureq_request(
                        &method_upper,
                        &url,
                        &headers,
                        body.as_deref(),
                        timeout,
                        max_redirects,
                        sink_batch,
                        &tx,
                        name,
                        tags,
                    ));
                }
                Ok(results)
            },
        ),
    )?;

    // Manual cookie-jar API, backed by the same per-worker store that handles
    // automatic cookies (http.cookieJar() in JS).
    let jar_obj = Object::new(ctx.clone())?;
    jar_obj.set("set", Function::new(ctx.clone(), jar_set))?;
    jar_obj.set("get", Function::new(ctx.clone(), jar_get))?;
    jar_obj.set(
        "cookiesForUrl",
        Function::new(ctx.clone(), jar_cookies_for_url),
    )?;
    jar_obj.set("delete", Function::new(ctx.clone(), jar_delete))?;
    jar_obj.set("clear", Function::new(ctx.clone(), jar_clear))?;
    http.set("__cookieJar", jar_obj)?;

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
                try { globalThis.__http_hooks.beforeRequest[i](req); } catch(e) { console.error('[Hook] beforeRequest error:', e); }
            }
        };
        globalThis.__http_callAfterResponseHooks = function(res) {
            for (var i = 0; i < globalThis.__http_hooks.afterResponse.length; i++) {
                try { globalThis.__http_hooks.afterResponse[i](res); } catch(e) { console.error('[Hook] afterResponse error:', e); }
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
        globalThis.__parseFileMarker = function(value) {
            if (typeof value !== 'string' || value.indexOf('__fusillade_file') === -1) return null;
            try {
                var parsed = JSON.parse(value);
                return (parsed && parsed.__fusillade_file) ? parsed : null;
            } catch (e) {
                return null;
            }
        };
        FormData.prototype.body = function() {
            var body = '';
            for (var i = 0; i < this._fields.length; i++) {
                var field = this._fields[i];
                body += '--' + this._boundary + '\r\n';
                var file = globalThis.__parseFileMarker(field.value);
                if (file) {
                    body += 'Content-Disposition: form-data; name="' + field.name + '"; filename="' + file.filename + '"\r\n';
                    body += 'Content-Type: ' + file.contentType + '\r\n\r\n';
                    body += file.content;
                } else {
                    body += 'Content-Disposition: form-data; name="' + field.name + '"\r\n\r\n';
                    body += field.value;
                }
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

        // Wrapper layer: global defaults (http.setDefaults), object-form
        // http.request, batch progress callbacks, and request/response hooks.
        // The native methods only see plain (url, body, options) calls.
        (function() {
            var nativeHttp = {
                get: http.get, post: http.post, put: http.put, del: http.del,
                patch: http.patch, head: http.head, options: http.options,
                request: http.request
            };
            var httpDefaults = { timeout: undefined, headers: undefined };

            http.setDefaults = function(opts) {
                opts = opts || {};
                if ('timeout' in opts) httpDefaults.timeout = opts.timeout;
                if ('headers' in opts) httpDefaults.headers = opts.headers;
            };

            function mergeDefaults(opts) {
                var merged = {};
                if (httpDefaults.headers) {
                    merged.headers = {};
                    for (var dk in httpDefaults.headers) merged.headers[dk] = httpDefaults.headers[dk];
                }
                if (httpDefaults.timeout !== undefined) merged.timeout = httpDefaults.timeout;
                if (opts) {
                    for (var k in opts) {
                        if (k === 'headers' && merged.headers) {
                            for (var hk in opts.headers) merged.headers[hk] = opts.headers[hk];
                        } else {
                            merged[k] = opts[k];
                        }
                    }
                }
                return merged;
            }

            function callNative(method, url, body, opts) {
                switch (method) {
                    case 'GET': return nativeHttp.get(url, opts);
                    case 'POST': return nativeHttp.post(url, body == null ? '' : body, opts);
                    case 'PUT': return nativeHttp.put(url, body == null ? '' : body, opts);
                    case 'PATCH': return nativeHttp.patch(url, body == null ? '' : body, opts);
                    case 'DELETE': return body == null
                        ? nativeHttp.del(url, opts)
                        : nativeHttp.request('DELETE', url, body, opts);
                    case 'HEAD': return nativeHttp.head(url, opts);
                    case 'OPTIONS': return nativeHttp.options(url, opts);
                    default: return nativeHttp.request(method, url, body, opts);
                }
            }

            function dispatch(method, url, body, opts) {
                var merged = mergeDefaults(opts);
                var req = {
                    method: String(method || 'GET').toUpperCase(),
                    url: url,
                    body: body,
                    headers: merged.headers || {}
                };
                // Hooks may mutate url, body, and headers before the request runs.
                globalThis.__http_callBeforeRequestHooks(req);
                // A bare http.file() marker body becomes a one-part multipart upload.
                var file = globalThis.__parseFileMarker(req.body);
                if (file) {
                    var fd = new FormData();
                    fd.append('file', req.body);
                    req.body = fd.body();
                    req.headers['Content-Type'] = fd.contentType();
                }
                merged.headers = req.headers;
                var res = callNative(req.method, req.url, req.body, merged);

                // Documented retry options: retry on network failure by
                // default (status 0), or on a custom retryOn(res) predicate,
                // with exponential backoff (retryDelay doubles, capped 32x).
                var retries = typeof merged.retry === 'number' ? merged.retry : 0;
                for (var attempt = 1; attempt <= retries; attempt++) {
                    var shouldRetry = typeof merged.retryOn === 'function'
                        ? merged.retryOn(res)
                        : res.status === 0;
                    if (!shouldRetry) break;
                    var delayMs;
                    if (typeof merged.retryDelayFn === 'function') {
                        delayMs = merged.retryDelayFn(attempt);
                    } else {
                        var base = typeof merged.retryDelay === 'number' ? merged.retryDelay : 100;
                        delayMs = base * Math.min(Math.pow(2, attempt - 1), 32);
                    }
                    if (delayMs > 0) sleep(delayMs / 1000);
                    res = callNative(req.method, req.url, req.body, merged);
                }

                globalThis.__http_callAfterResponseHooks(res);
                return res;
            }

            function optionsFromRequestObject(r) {
                var o = {};
                for (var k in r) {
                    if (k !== 'method' && k !== 'url' && k !== 'body') o[k] = r[k];
                }
                return o;
            }

            http.get = function(url, opts) { return dispatch('GET', url, undefined, opts); };
            http.post = function(url, body, opts) { return dispatch('POST', url, body, opts); };
            http.put = function(url, body, opts) { return dispatch('PUT', url, body, opts); };
            http.patch = function(url, body, opts) { return dispatch('PATCH', url, body, opts); };
            http.del = function(url, opts) { return dispatch('DELETE', url, undefined, opts); };
            http.head = function(url, opts) { return dispatch('HEAD', url, undefined, opts); };
            http.options = function(url, opts) { return dispatch('OPTIONS', url, undefined, opts); };

            http.request = function(methodOrReq, url, body, opts) {
                if (methodOrReq && typeof methodOrReq === 'object') {
                    var r = methodOrReq;
                    return dispatch(r.method, r.url, r.body, optionsFromRequestObject(r));
                }
                return dispatch(methodOrReq, url, body, opts);
            };

            http.batch = function(requests, onProgress) {
                var results = [];
                for (var i = 0; i < requests.length; i++) {
                    var r = requests[i];
                    results.push(dispatch(r.method, r.url, r.body, optionsFromRequestObject(r)));
                    if (typeof onProgress === 'function') {
                        try { onProgress(i + 1, requests.length); } catch (e) {}
                    }
                }
                return results;
            };

            http.url = function(base, params) {
                if (!params) return base;
                var parts = [];
                for (var k in params) {
                    parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(params[k]));
                }
                if (parts.length === 0) return base;
                return base + (base.indexOf('?') === -1 ? '?' : '&') + parts.join('&');
            };

            http.formEncode = function(obj) {
                var parts = [];
                for (var k in obj) {
                    parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(obj[k]));
                }
                return parts.join('&');
            };

            http.basicAuth = function(username, password) {
                return 'Basic ' + encoding.b64encode(username + ':' + password);
            };

            http.bearerToken = function(token) {
                return 'Bearer ' + token;
            };

            http.cookieJar = function() {
                return http.__cookieJar;
            };

            // Reads a file and returns a JSON marker; FormData.body() and
            // http.post() expand markers into multipart file parts.
            http.file = function(path, filename, contentType) {
                return JSON.stringify({
                    __fusillade_file: true,
                    content: open(path),
                    filename: filename || String(path).split('/').pop(),
                    contentType: contentType || 'application/octet-stream'
                });
            };
        })();
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
