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
use rquickjs::{Ctx, Function, IntoJs, Object, Result, Value};
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
        obj.set("status", self.status)?;
        obj.set("statusText", &self.status_text)?;
        obj.set("proto", &self.proto)?;

        let body_str = String::from_utf8_lossy(&self.body);
        let body_string = body_str.to_string();
        obj.set("body", body_str.as_ref())?;

        let headers_clone = self.headers.clone();
        let headers_obj = Object::new(ctx.clone())?;
        for (k, v) in self.headers {
            headers_obj.set(k, v)?;
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

        // Add json() method to parse body as JSON
        let ctx_clone = ctx.clone();
        obj.set(
            "json",
            Function::new(ctx.clone(), move || -> Result<Value<'_>> {
                let json_global: Object = ctx_clone.globals().get("JSON")?;
                let parse: Function = json_global.get("parse")?;
                parse.call((body_string.clone(),))
            }),
        )?;

        // Add bodyContains(str) method
        let body_for_contains = body_str.to_string();
        obj.set(
            "bodyContains",
            Function::new(ctx.clone(), move |needle: String| -> Result<bool> {
                Ok(body_for_contains.contains(&needle))
            }),
        )?;

        // Add bodyMatches(regex) method
        let body_for_matches = body_str.to_string();
        obj.set(
            "bodyMatches",
            Function::new(ctx.clone(), move |pattern: String| -> Result<bool> {
                match regex::Regex::new(&pattern) {
                    Ok(re) => Ok(re.is_match(&body_for_matches)),
                    Err(_) => Ok(false),
                }
            }),
        )?;

        // Add hasHeader(name, value?) method - check if header exists (and optionally matches value)
        let headers_for_check = headers_clone.clone();
        obj.set(
            "hasHeader",
            Function::new(
                ctx.clone(),
                move |name: String, value: Option<String>| -> bool {
                    match headers_for_check.get(&name) {
                        Some(v) => {
                            if let Some(expected) = value {
                                v == &expected
                            } else {
                                true
                            }
                        }
                        None => {
                            // Try case-insensitive lookup
                            let name_lower = name.to_lowercase();
                            headers_for_check.iter().any(|(k, v)| {
                                if k.to_lowercase() == name_lower {
                                    if let Some(ref expected) = value {
                                        v == expected
                                    } else {
                                        true
                                    }
                                } else {
                                    false
                                }
                            })
                        }
                    }
                },
            ),
        )?;

        // Add isJson() method
        let headers_for_json = headers_clone;
        obj.set(
            "isJson",
            Function::new(ctx.clone(), move || -> Result<bool> {
                Ok(headers_for_json
                    .get("content-type")
                    .or_else(|| headers_for_json.get("Content-Type"))
                    .map(|ct| ct.contains("application/json"))
                    .unwrap_or(false))
            }),
        )?;

        // Add cookies property - parse Set-Cookie headers
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
                        // When response_sink is enabled, read and discard the body to save memory
                        // Body must still be read for connection keep-alive
                        let body = if sink_get {
                            // Read and discard - consume the response but don't store
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

                        // Use Cow to avoid allocation when metric_name equals url_str
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

    ctx.globals().set("http", http)?;
    Ok(())
}
