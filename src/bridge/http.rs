use crate::engine::http_client::HttpClient;
use crate::stats::{Metric, RequestTimings};
use cookie_store::CookieStore;
use crossbeam_channel::Sender;
use http::{Method, Request, Uri};
use hyper::body::Bytes;
use rand::Rng;
use rquickjs::{Ctx, Function, IntoJs, Object, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::runtime::Runtime;
use url::Url;

/// Global HTTP request defaults that can be set via http.setDefaults()
#[derive(Default, Clone)]
struct HttpDefaults {
    timeout: Option<Duration>,
    headers: HashMap<String, String>,
}

/// Parse a duration string (e.g., "30s", "500ms", "1m") into std::time::Duration
fn parse_duration_str(s: &str) -> Option<Duration> {
    if s.ends_with("ms") {
        s.trim_end_matches("ms")
            .parse::<u64>()
            .ok()
            .map(Duration::from_millis)
    } else if s.ends_with('s') {
        s.trim_end_matches('s')
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
    } else if s.ends_with('m') {
        s.trim_end_matches('m')
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else {
        // Try parsing as milliseconds number
        s.parse::<u64>().ok().map(Duration::from_millis)
    }
}

/// Convert a url::Url to http::Uri using component extraction.
/// This avoids the full string re-parsing that Uri::try_from(String) performs.
fn url_to_uri(url: &Url) -> Option<Uri> {
    let scheme = url.scheme();
    let host = url.host_str()?;

    // Build authority: host[:port]
    let authority = if let Some(port) = url.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    // Build path_and_query
    let path = url.path();
    let path_and_query = if let Some(query) = url.query() {
        format!("{}?{}", path, query)
    } else {
        path.to_string()
    };

    Uri::builder()
        .scheme(scheme)
        .authority(authority.as_str())
        .path_and_query(path_and_query.as_str())
        .build()
        .ok()
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Bytes,
    pub headers: HashMap<String, String>,
    pub timings: RequestTimings,
    pub proto: String,
}

impl<'js> IntoJs<'js> for HttpResponse {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("status", self.status)?;
        obj.set("proto", self.proto)?;

        // Defer String conversion to here (avoiding intermediate Rust String if possible by strictly using ref)
        // String::from_utf8_lossy returns Cow<str>. If it's a slice (valid utf8), we pass &str.
        // If it's owned (replacement chars), we pass String. Both implement IntoJs.
        let body_str = String::from_utf8_lossy(&self.body);
        let body_string = body_str.to_string();
        obj.set("body", body_str.as_ref())?;

        // Clone headers for use in hasHeader method before moving into JS object
        let headers_for_check = self.headers.clone();

        let headers_obj = Object::new(ctx.clone())?;
        for (k, v) in self.headers {
            headers_obj.set(k, v)?;
        }
        obj.set("headers", headers_obj)?;

        // Add timings object (k6-compatible)
        let timings_obj = Object::new(ctx.clone())?;
        timings_obj.set("duration", self.timings.duration.as_secs_f64() * 1000.0)?;
        timings_obj.set("blocked", self.timings.blocked.as_secs_f64() * 1000.0)?;
        timings_obj.set("connecting", self.timings.connecting.as_secs_f64() * 1000.0)?;
        timings_obj.set(
            "tls_handshaking",
            self.timings.tls_handshaking.as_secs_f64() * 1000.0,
        )?;
        timings_obj.set("sending", self.timings.sending.as_secs_f64() * 1000.0)?;
        timings_obj.set("waiting", self.timings.waiting.as_secs_f64() * 1000.0)?;
        timings_obj.set("receiving", self.timings.receiving.as_secs_f64() * 1000.0)?;
        obj.set("timings", timings_obj)?;

        // Add json() method to parse body as JSON
        let ctx_clone = ctx.clone();
        let body_for_json = body_string.clone();
        obj.set(
            "json",
            Function::new(ctx.clone(), move || -> Result<Value<'_>> {
                let json_global: Object = ctx_clone.globals().get("JSON")?;
                let parse: Function = json_global.get("parse")?;
                parse.call((body_for_json.clone(),))
            }),
        )?;

        // Add bodyContains(str) method - check if body contains string
        let body_for_contains = body_string.clone();
        obj.set(
            "bodyContains",
            Function::new(ctx.clone(), move |needle: String| -> bool {
                body_for_contains.contains(&needle)
            }),
        )?;

        // Add bodyMatches(pattern) method - check if body matches regex
        let body_for_matches = body_string.clone();
        obj.set(
            "bodyMatches",
            Function::new(ctx.clone(), move |pattern: String| -> bool {
                match regex::Regex::new(&pattern) {
                    Ok(re) => re.is_match(&body_for_matches),
                    Err(_) => false,
                }
            }),
        )?;

        // Add hasHeader(name, value?) method - check if header exists (and optionally matches value)
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

        // Add isJson() method - check if body is valid JSON
        let body_for_is_json = body_string.clone();
        obj.set(
            "isJson",
            Function::new(ctx.clone(), move || -> bool {
                serde_json::from_str::<serde_json::Value>(&body_for_is_json).is_ok()
            }),
        )?;

        Ok(obj.into_value())
    }
}

/// Convert HTTP version to short string (h1, h2, h3)
fn version_to_proto(version: http::Version) -> String {
    match version {
        http::Version::HTTP_09 => "h1".to_string(),
        http::Version::HTTP_10 => "h1".to_string(),
        http::Version::HTTP_11 => "h1".to_string(),
        http::Version::HTTP_2 => "h2".to_string(),
        http::Version::HTTP_3 => "h3".to_string(),
        _ => "h1".to_string(),
    }
}

pub fn register_sync(
    ctx: &Ctx,
    tx: Sender<Metric>,
    client: HttpClient,
    runtime: Arc<Runtime>,
    jitter_str: Option<String>,
    drop_prob: Option<f64>,
    response_sink: bool,
) -> Result<()> {
    // Parse jitter once
    let global_jitter = jitter_str.as_deref().and_then(parse_duration_str);
    let global_drop = drop_prob;
    let global_response_sink = response_sink;
    let http = Object::new(ctx.clone())?;
    let cookie_store = std::rc::Rc::new(RefCell::new(CookieStore::default()));
    let defaults = std::rc::Rc::new(RefCell::new(HttpDefaults::default()));

    let c_base = client.clone();
    let tx_base = tx.clone();
    let cs_base = cookie_store.clone();
    let rt_base = runtime.clone();
    let defaults_base = defaults.clone();

    http.set(
        "request",
        Function::new(ctx.clone(), move |opts: Object| -> Result<HttpResponse> {
            let client = c_base.clone();
            let tx = tx_base.clone();
            let runtime = rt_base.clone();
            let jitter = global_jitter;
            let drop = global_drop;
            let response_sink = global_response_sink;

            // Get defaults
            let defs = defaults_base.borrow();

            let mut method_str: String = opts.get("method").unwrap_or_else(|_| "GET".to_string());
            let mut url_str: String = opts
                .get("url")
                .map_err(|_| rquickjs::Error::new_from_js("url is required", "ValueError"))?;
            let name_opt: Option<String> = opts.get("name").ok();
            let body: Option<String> = opts.get("body").ok();
            let request_headers: Option<HashMap<String, String>> = opts.get("headers").ok();

            // Merge default headers with request headers (request headers take precedence)
            let headers_map: Option<HashMap<String, String>> = if defs.headers.is_empty() {
                request_headers
            } else {
                let mut merged = defs.headers.clone();
                if let Some(req_h) = request_headers {
                    for (k, v) in req_h {
                        merged.insert(k, v);
                    }
                }
                Some(merged)
            };

            let mut tags: HashMap<String, String> = opts.get("tags").unwrap_or_default();
            // Auto-inject scenario tag from global if available
            let ctx = opts.ctx();
            if let Ok(scenario_name) = ctx.globals().get::<_, String>("__SCENARIO") {
                tags.insert("scenario".to_string(), scenario_name);
            }

            // Use default timeout if not specified in request
            let mut timeout_duration: Option<Duration> =
                defs.timeout.or(Some(Duration::from_secs(60)));
            if let Ok(timeout_str) = opts.get::<_, String>("timeout") {
                timeout_duration = parse_duration_str(&timeout_str);
            } else if let Ok(timeout_ms) = opts.get::<_, u64>("timeout") {
                timeout_duration = Some(Duration::from_millis(timeout_ms));
            }

            // Drop the borrow before the async block
            std::mem::drop(defs);

            // Retry configuration
            let max_retries: u32 = opts.get("retry").unwrap_or(0);
            let retry_delay_ms: u64 = opts.get("retryDelay").unwrap_or(100);
            let mut retry_count: u32 = 0;

            // Redirect configuration
            let follow_redirects: bool = opts.get("followRedirects").unwrap_or(true);
            let max_redirects: u32 = opts.get("maxRedirects").unwrap_or(5);

            let mut redirects = 0;
            loop {
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let method =
                    Method::from_bytes(method_str.to_uppercase().as_bytes()).unwrap_or(Method::GET);

                let mut req_builder = Request::builder()
                    .method(method.clone())
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);

                // Cookies
                let mut cookie_header_val = String::new();
                {
                    let store = cs_base.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }

                // Headers
                if let Some(h) = &headers_map {
                    for (k, v) in h {
                        req_builder = req_builder.header(k, v);
                    }
                }

                let final_body = if redirects == 0 {
                    body.clone().unwrap_or_default()
                } else {
                    String::new()
                };

                let req = req_builder
                    .body(final_body)
                    .map_err(|_| rquickjs::Error::Exception)?;

                // Execute via Runtime
                let start = Instant::now(); // Fallback start time
                                            // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    // Chaos: Drop
                    if let Some(d) = drop {
                        if rand::thread_rng().gen::<f64>() < d {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                "Simulated network drop",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }
                    // Chaos: Jitter
                    if let Some(j) = jitter {
                        tokio::time::sleep(j).await;
                    }

                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());

                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        let mut set_cookie_headers: Vec<String> = Vec::new();
                        let mut location_header = None;

                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                let name_str = name.to_string();
                                resp_headers.insert(name_str.clone(), val_str.to_string());
                                let lower = name_str.to_lowercase();
                                if lower == "set-cookie" {
                                    set_cookie_headers.push(val_str.to_string());
                                }
                                if lower == "location" {
                                    location_header = Some(val_str.to_string());
                                }
                            }
                        }

                        // Body is already read in client.request and inside response
                        // When response_sink is enabled, body will be empty (Bytes::new())
                        let body = response.body().clone();

                        let metric_name = name_opt.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });

                        {
                            let mut store = cs_base.borrow_mut();
                            for cookie_val in set_cookie_headers {
                                let _ = store.parse(&cookie_val, &url_parsed);
                            }
                        }

                        if follow_redirects
                            && (300..400).contains(&status)
                            && redirects < max_redirects
                        {
                            if let Some(loc_str) = location_header {
                                let next_url = url_parsed.join(&loc_str).map_err(|_| {
                                    rquickjs::Error::new_from_js("Invalid Redirect URL", "UrlError")
                                })?;
                                url_str = next_url.to_string();
                                redirects += 1;
                                method_str = "GET".to_string();
                                continue;
                            }
                        }

                        return Ok(HttpResponse {
                            status,
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                        });
                    }
                    Err(e) => {
                        // Retry logic with exponential backoff
                        if retry_count < max_retries {
                            retry_count += 1;
                            let delay = Duration::from_millis(
                                retry_delay_ms * (1 << (retry_count - 1).min(5)),
                            );
                            runtime.block_on(async {
                                tokio::time::sleep(delay).await;
                            });
                            redirects = 0; // Reset redirects for retry
                            continue;
                        }

                        let duration = start.elapsed();
                        let metric_name = name_opt.clone().unwrap_or_else(|| url_str.clone());

                        // Fallback timings for error case
                        let timings = RequestTimings {
                            duration,
                            ..Default::default()
                        };

                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        return Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        });
                    }
                }
            }
        }),
    )?;

    // Helper GET
    let c_get = client.clone();
    let tx_get = tx.clone();
    let cs_get = cookie_store.clone();
    let rt_get = runtime.clone();
    let sink_get = global_response_sink;

    http.set(
        "get",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };

                let client = c_get.clone();
                let tx = tx_get.clone();
                let runtime = rt_get.clone();
                let jitter = global_jitter;
                let drop = global_drop;
                let response_sink = sink_get;

                let mut current_url = url_str.clone();
                let mut redirects = 0;

                loop {
                    let url_parsed = Url::parse(&current_url)
                        .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

                    let mut req_builder = Request::builder()
                        .method(Method::GET)
                        .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);

                    // Cookies
                    let mut cookie_header_val = String::new();
                    {
                        let store = cs_get.borrow();
                        for (name, value) in store.get_request_values(&url_parsed) {
                            if !cookie_header_val.is_empty() {
                                cookie_header_val.push_str("; ");
                            }
                            cookie_header_val.push_str(name);
                            cookie_header_val.push('=');
                            cookie_header_val.push_str(value);
                        }
                    }
                    if !cookie_header_val.is_empty() {
                        // eprintln!("DEBUG: Sending Cookie to {}: {}", url_parsed.as_str(), cookie_header_val);
                        req_builder = req_builder.header("Cookie", cookie_header_val);
                    }

                    let req = req_builder
                        .body(String::new())
                        .map_err(|_| rquickjs::Error::Exception)?;

                    let start = Instant::now();

                    // Execute request with optional timeout
                    let res_result = runtime.block_on(async {
                        if let Some(d) = drop {
                            if rand::thread_rng().gen::<f64>() < d {
                                return Err(Box::new(std::io::Error::new(
                                    std::io::ErrorKind::ConnectionReset,
                                    "Simulated network drop",
                                ))
                                    as Box<dyn std::error::Error + Send + Sync>);
                            }
                        }
                        if let Some(j) = jitter {
                            tokio::time::sleep(j).await;
                        }

                        let request_future = client.request(req, response_sink);
                        if let Some(timeout) = timeout_duration {
                            match tokio::time::timeout(timeout, request_future).await {
                                Ok(result) => result,
                                Err(_) => Err(Box::new(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "request timeout",
                                ))
                                    as Box<dyn std::error::Error + Send + Sync>),
                            }
                        } else {
                            request_future.await
                        }
                    });

                    match res_result {
                        Ok((response, timings)) => {
                            let status = response.status().as_u16();
                            let proto = version_to_proto(response.version());
                            let mut resp_headers: HashMap<String, String> =
                                HashMap::with_capacity(response.headers().len());
                            let mut set_cookie_headers: Vec<String> = Vec::new();
                            let mut location_header = None;

                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    resp_headers.insert(name_str.clone(), val_str.to_string());
                                    let lower = name_str.to_lowercase();
                                    if lower == "set-cookie" {
                                        set_cookie_headers.push(val_str.to_string());
                                    }
                                    if lower == "location" {
                                        location_header = Some(val_str.to_string());
                                    }
                                }
                            }
                            let body = response.body().clone();

                            let metric_name =
                                name_tag.clone().unwrap_or_else(|| current_url.clone());
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status,
                                error: None,
                                tags: tags.clone(),
                            });

                            {
                                let mut store = cs_get.borrow_mut();
                                for cookie_val in set_cookie_headers {
                                    if store.parse(&cookie_val, &url_parsed).is_ok() {}
                                }
                            }

                            if (300..400).contains(&status) && redirects < 5 {
                                if let Some(loc_str) = location_header {
                                    let next_url = url_parsed.join(&loc_str).map_err(|_| {
                                        rquickjs::Error::new_from_js(
                                            "Invalid Redirect URL",
                                            "UrlError",
                                        )
                                    })?;
                                    current_url = next_url.to_string();
                                    redirects += 1;
                                    continue;
                                }
                            }
                            return Ok(HttpResponse {
                                status,
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                            });
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let metric_name =
                                name_tag.clone().unwrap_or_else(|| current_url.clone());
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.to_string()),
                                tags: tags.clone(),
                            });
                            return Ok(HttpResponse {
                                status: 0,
                                body: Bytes::from(e.to_string()),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                            });
                        }
                    }
                }
            },
        ),
    )?;

    // Helper POST with FormData detection
    let c_post = client.clone();
    let tx_post = tx.clone();
    let cs_post = cookie_store.clone();
    let rt_post = runtime.clone();
    let sink_post = global_response_sink;

    http.set(
        "post",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag = None;
                let mut headers_map: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        headers_map = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };

                let client = c_post.clone();
                let tx = tx_post.clone();
                let runtime = rt_post.clone();
                let jitter = global_jitter;
                let drop = global_drop;
                let response_sink = sink_post;

                let mut current_url = url_str.clone();
                let mut redirects = 0;
                let mut current_method = Method::POST;

                // Check if body contains file markers (FormData detection)
                let is_multipart = body.contains("\"__fusillade_file\":true");

                // For multipart, we need to use reqwest blocking client directly
                if is_multipart {
                    // Parse body as JSON object
                    if let Ok(form_data) = serde_json::from_str::<serde_json::Value>(&body) {
                        if let Some(obj) = form_data.as_object() {
                            let mut client_builder = reqwest::blocking::Client::builder();
                            if let Some(timeout) = timeout_duration {
                                client_builder = client_builder.timeout(timeout);
                            }
                            let blocking_client = client_builder
                                .build()
                                .unwrap_or_else(|_| reqwest::blocking::Client::new());
                            let mut form = reqwest::blocking::multipart::Form::new();

                            for (key, value) in obj {
                                if let Some(file_obj) = value.as_object() {
                                    if file_obj
                                        .get("__fusillade_file")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false)
                                    {
                                        // This is a file field
                                        let data_b64 = file_obj
                                            .get("data")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let filename = file_obj
                                            .get("filename")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("file");
                                        let content_type = file_obj
                                            .get("contentType")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("application/octet-stream");

                                        use base64::Engine;
                                        if let Ok(data) = base64::engine::general_purpose::STANDARD
                                            .decode(data_b64)
                                        {
                                            let part =
                                                reqwest::blocking::multipart::Part::bytes(data)
                                                    .file_name(filename.to_string())
                                                    .mime_str(content_type)
                                                    .unwrap_or_else(|_| {
                                                        reqwest::blocking::multipart::Part::bytes(
                                                            vec![],
                                                        )
                                                    });
                                            form = form.part(key.clone(), part);
                                        }
                                    }
                                } else if let Some(s) = value.as_str() {
                                    // Regular form field
                                    form = form.text(key.clone(), s.to_string());
                                }
                            }

                            let start = std::time::Instant::now();
                            // NOTE: Blocking client wrapper for multipart doesn't easily support async sleep inside unless we wrap it
                            // But we used reqwest::blocking::Client.
                            // We can use std::thread::sleep here as it's a blocking client call on a dedicated thread anyway?
                            // No, runtime.block_on is not used here for multipart?
                            // Wait, code says: reqwest::blocking::Client::builder()...
                            // And then blocking_client.post()... .send()
                            // This is synchronous code strictly.

                            if let Some(d) = drop {
                                if rand::thread_rng().gen::<f64>() < d {
                                    return Err(rquickjs::Error::new_from_js(
                                        "Simulated network drop",
                                        "NetworkError",
                                    ));
                                }
                            }
                            if let Some(j) = jitter {
                                std::thread::sleep(j);
                            }

                            let res = blocking_client.post(&current_url).multipart(form).send();
                            let duration = start.elapsed();

                            match res {
                                Ok(response) => {
                                    let status = response.status().as_u16();
                                    let proto = version_to_proto(response.version());
                                    let mut resp_headers: HashMap<String, String> =
                                        HashMap::with_capacity(response.headers().len());
                                    for (name, val) in response.headers() {
                                        if let Ok(val_str) = val.to_str() {
                                            resp_headers
                                                .insert(name.to_string(), val_str.to_string());
                                        }
                                    }
                                    let body_bytes = response.bytes().unwrap_or_default();

                                    let timings = RequestTimings {
                                        duration,
                                        ..Default::default()
                                    };
                                    let metric_name =
                                        name_tag.clone().unwrap_or_else(|| current_url.clone());
                                    let _ = tx.send(Metric::Request {
                                        name: format!(
                                            "{}{}",
                                            crate::bridge::group::get_current_group_prefix(),
                                            metric_name
                                        ),
                                        timings,
                                        status,
                                        error: None,
                                        tags: tags.clone(),
                                    });

                                    return Ok(HttpResponse {
                                        status,
                                        body: Bytes::from(body_bytes.to_vec()),
                                        headers: resp_headers,
                                        timings,
                                        proto,
                                    });
                                }
                                Err(e) => {
                                    let timings = RequestTimings {
                                        duration,
                                        ..Default::default()
                                    };
                                    let metric_name =
                                        name_tag.clone().unwrap_or_else(|| current_url.clone());
                                    let _ = tx.send(Metric::Request {
                                        name: format!(
                                            "{}{}",
                                            crate::bridge::group::get_current_group_prefix(),
                                            metric_name
                                        ),
                                        timings,
                                        status: 0,
                                        error: Some(e.to_string()),
                                        tags: tags.clone(),
                                    });
                                    return Ok(HttpResponse {
                                        status: 0,
                                        body: Bytes::from(e.to_string()),
                                        headers: HashMap::new(),
                                        timings,
                                        proto: "h1".to_string(),
                                    });
                                }
                            }
                        }
                    }
                }

                // Standard POST (non-multipart)
                let mut current_body = Some(body);

                loop {
                    let url_parsed = Url::parse(&current_url)
                        .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

                    let mut req_builder = Request::builder()
                        .method(current_method.clone())
                        .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);

                    // Cookies
                    let mut cookie_header_val = String::new();
                    {
                        let store = cs_post.borrow();
                        for (name, value) in store.get_request_values(&url_parsed) {
                            if !cookie_header_val.is_empty() {
                                cookie_header_val.push_str("; ");
                            }
                            cookie_header_val.push_str(name);
                            cookie_header_val.push('=');
                            cookie_header_val.push_str(value);
                        }
                    }
                    if !cookie_header_val.is_empty() {
                        req_builder = req_builder.header("Cookie", cookie_header_val);
                    }
                    // Apply custom headers
                    if let Some(ref h) = headers_map {
                        for (k, v) in h {
                            req_builder = req_builder.header(k, v);
                        }
                    }

                    let final_body = current_body.clone().unwrap_or_default();
                    let req = req_builder
                        .body(final_body)
                        .map_err(|_| rquickjs::Error::Exception)?;

                    let start = Instant::now();
                    // Execute request with optional timeout
                    let res_result = runtime.block_on(async {
                        if let Some(d) = drop {
                            if rand::thread_rng().gen::<f64>() < d {
                                return Err(Box::new(std::io::Error::new(
                                    std::io::ErrorKind::ConnectionReset,
                                    "Simulated network drop",
                                ))
                                    as Box<dyn std::error::Error + Send + Sync>);
                            }
                        }
                        if let Some(j) = jitter {
                            tokio::time::sleep(j).await;
                        }

                        let request_future = client.request(req, response_sink);
                        if let Some(timeout) = timeout_duration {
                            match tokio::time::timeout(timeout, request_future).await {
                                Ok(result) => result,
                                Err(_) => Err(Box::new(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "request timeout",
                                ))
                                    as Box<dyn std::error::Error + Send + Sync>),
                            }
                        } else {
                            request_future.await
                        }
                    });

                    match res_result {
                        Ok((response, timings)) => {
                            let status = response.status().as_u16();
                            let proto = version_to_proto(response.version());
                            let mut resp_headers: HashMap<String, String> =
                                HashMap::with_capacity(response.headers().len());
                            let mut set_cookie_headers: Vec<String> = Vec::new();
                            let mut location_header = None;

                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    resp_headers.insert(name_str.clone(), val_str.to_string());
                                    let lower = name_str.to_lowercase();
                                    if lower == "set-cookie" {
                                        set_cookie_headers.push(val_str.to_string());
                                    }
                                    if lower == "location" {
                                        location_header = Some(val_str.to_string());
                                    }
                                }
                            }

                            let body = response.body().clone();

                            let metric_name =
                                name_tag.clone().unwrap_or_else(|| current_url.clone());
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status,
                                error: None,
                                tags: tags.clone(),
                            });

                            {
                                let mut store = cs_post.borrow_mut();
                                for cookie_val in set_cookie_headers {
                                    let _ = store.parse(&cookie_val, &url_parsed);
                                }
                            }

                            if (300..400).contains(&status) && redirects < 5 {
                                if let Some(loc_str) = location_header {
                                    let next_url = url_parsed.join(&loc_str).map_err(|_| {
                                        rquickjs::Error::new_from_js(
                                            "Invalid Redirect URL",
                                            "UrlError",
                                        )
                                    })?;
                                    current_url = next_url.to_string();
                                    redirects += 1;
                                    current_method = Method::GET;
                                    current_body = None;
                                    continue;
                                }
                            }
                            return Ok(HttpResponse {
                                status,
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                            });
                        }
                        Err(e) => {
                            let duration = start.elapsed();
                            let metric_name =
                                name_tag.clone().unwrap_or_else(|| current_url.clone());
                            let timings = RequestTimings {
                                duration,
                                ..Default::default()
                            };
                            let _ = tx.send(Metric::Request {
                                name: format!(
                                    "{}{}",
                                    crate::bridge::group::get_current_group_prefix(),
                                    metric_name
                                ),
                                timings,
                                status: 0,
                                error: Some(e.to_string()),
                                tags: tags.clone(),
                            });
                            return Ok(HttpResponse {
                                status: 0,
                                body: Bytes::from(e.to_string()),
                                headers: HashMap::new(),
                                timings,
                                proto: "h1".to_string(),
                            });
                        }
                    }
                }
            },
        ),
    )?;

    // Helper PUT
    let c_put = client.clone();
    let tx_put = tx.clone();
    let cs_put = cookie_store.clone();
    let rt_put = runtime.clone();
    let sink_put = global_response_sink;

    http.set(
        "put",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag = None;
                let mut headers_map: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        headers_map = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };
                let client = c_put.clone();
                let tx = tx_put.clone();
                let runtime = rt_put.clone();
                let jitter = global_jitter;
                let drop = global_drop;
                let response_sink = sink_put;
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let mut req_builder = Request::builder()
                    .method(Method::PUT)
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);
                let mut cookie_header_val = String::new();
                {
                    let store = cs_put.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }
                if let Some(h) = &headers_map {
                    for (k, v) in h {
                        req_builder = req_builder.header(k, v);
                    }
                }
                let req = req_builder
                    .body(body)
                    .map_err(|_| rquickjs::Error::Exception)?;
                let start = Instant::now();
                // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    if let Some(d) = drop {
                        if rand::thread_rng().gen::<f64>() < d {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                "Simulated network drop",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }
                    if let Some(j) = jitter {
                        tokio::time::sleep(j).await;
                    }
                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());
                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                resp_headers.insert(name.to_string(), val_str.to_string());
                            }
                        }
                        let body = response.body().clone();
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status,
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        })
                    }
                }
            },
        ),
    )?;

    // Helper DELETE
    let c_del = client.clone();
    let tx_del = tx.clone();
    let cs_del = cookie_store.clone();
    let rt_del = runtime.clone();
    let sink_del = global_response_sink;

    http.set(
        "del",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag: Option<String> = None;
                let mut headers_map: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        headers_map = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };
                let client = c_del.clone();
                let tx = tx_del.clone();
                let runtime = rt_del.clone();
                let jitter = global_jitter;
                let drop = global_drop;
                let response_sink = sink_del;
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let mut req_builder = Request::builder()
                    .method(Method::DELETE)
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);
                let mut cookie_header_val = String::new();
                {
                    let store = cs_del.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }
                if let Some(h) = &headers_map {
                    for (k, v) in h {
                        req_builder = req_builder.header(k, v);
                    }
                }
                let req = req_builder
                    .body(String::new())
                    .map_err(|_| rquickjs::Error::Exception)?;
                let start = Instant::now();
                // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    if let Some(d) = drop {
                        if rand::thread_rng().gen::<f64>() < d {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                "Simulated network drop",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }
                    if let Some(j) = jitter {
                        tokio::time::sleep(j).await;
                    }
                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());
                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                resp_headers.insert(name.to_string(), val_str.to_string());
                            }
                        }
                        let body = response.body().clone();
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status,
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        })
                    }
                }
            },
        ),
    )?;

    // Helper PATCH
    let c_patch = client.clone();
    let tx_patch = tx.clone();
    let cs_patch = cookie_store.clone();
    let rt_patch = runtime.clone();
    let sink_patch = global_response_sink;

    http.set(
        "patch",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag: Option<String> = None;
                let mut headers_map: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        headers_map = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };
                let client = c_patch.clone();
                let tx = tx_patch.clone();
                let runtime = rt_patch.clone();
                let jitter = global_jitter;
                let drop = global_drop;
                let response_sink = sink_patch;
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let mut req_builder = Request::builder()
                    .method(Method::PATCH)
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);
                let mut cookie_header_val = String::new();
                {
                    let store = cs_patch.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }
                if let Some(h) = &headers_map {
                    for (k, v) in h {
                        req_builder = req_builder.header(k, v);
                    }
                }
                let req = req_builder
                    .body(body)
                    .map_err(|_| rquickjs::Error::Exception)?;
                let start = Instant::now();
                // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    if let Some(d) = drop {
                        if rand::thread_rng().gen::<f64>() < d {
                            return Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::ConnectionReset,
                                "Simulated network drop",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>);
                        }
                    }
                    if let Some(j) = jitter {
                        tokio::time::sleep(j).await;
                    }
                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());
                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                resp_headers.insert(name.to_string(), val_str.to_string());
                            }
                        }
                        let body = response.body().clone();
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status,
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        })
                    }
                }
            },
        ),
    )?;

    // Helper HEAD
    let c_head = client.clone();
    let tx_head = tx.clone();
    let cs_head = cookie_store.clone();
    let rt_head = runtime.clone();
    let sink_head = global_response_sink;

    http.set(
        "head",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag: Option<String> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };
                let client = c_head.clone();
                let tx = tx_head.clone();
                let runtime = rt_head.clone();
                let response_sink = sink_head;
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let mut req_builder = Request::builder()
                    .method(Method::HEAD)
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);
                let mut cookie_header_val = String::new();
                {
                    let store = cs_head.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }
                let req = req_builder
                    .body(String::new())
                    .map_err(|_| rquickjs::Error::Exception)?;
                let start = Instant::now();
                // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());
                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                resp_headers.insert(name.to_string(), val_str.to_string());
                            }
                        }
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status,
                            body: Bytes::new(),
                            headers: resp_headers,
                            timings,
                            proto,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        })
                    }
                }
            },
        ),
    )?;

    // Helper OPTIONS
    let c_opts = client.clone();
    let tx_opts = tx.clone();
    let cs_opts = cookie_store.clone();
    let rt_opts = runtime.clone();
    let sink_opts = global_response_sink;

    http.set(
        "options",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                let mut name_tag: Option<String> = None;
                let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }
                let tags: HashMap<String, String> = if let Some(arg) = rest.first() {
                    arg.as_object()
                        .and_then(|o| o.get("tags").ok())
                        .unwrap_or_default()
                } else {
                    HashMap::new()
                };
                let client = c_opts.clone();
                let tx = tx_opts.clone();
                let runtime = rt_opts.clone();
                let response_sink = sink_opts;
                let url_parsed = Url::parse(&url_str)
                    .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                let mut req_builder = Request::builder()
                    .method(Method::OPTIONS)
                    .uri(url_to_uri(&url_parsed).ok_or(rquickjs::Error::Exception)?);
                let mut cookie_header_val = String::new();
                {
                    let store = cs_opts.borrow();
                    for (name, value) in store.get_request_values(&url_parsed) {
                        if !cookie_header_val.is_empty() {
                            cookie_header_val.push_str("; ");
                        }
                        cookie_header_val.push_str(name);
                        cookie_header_val.push('=');
                        cookie_header_val.push_str(value);
                    }
                }
                if !cookie_header_val.is_empty() {
                    req_builder = req_builder.header("Cookie", cookie_header_val);
                }
                let req = req_builder
                    .body(String::new())
                    .map_err(|_| rquickjs::Error::Exception)?;
                let start = Instant::now();
                // Execute request with optional timeout
                let res_result = runtime.block_on(async {
                    let request_future = client.request(req, response_sink);
                    if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => Err(Box::new(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "request timeout",
                            ))
                                as Box<dyn std::error::Error + Send + Sync>),
                        }
                    } else {
                        request_future.await
                    }
                });

                match res_result {
                    Ok((response, timings)) => {
                        let status = response.status().as_u16();
                        let proto = version_to_proto(response.version());
                        let mut resp_headers: HashMap<String, String> =
                            HashMap::with_capacity(response.headers().len());
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                resp_headers.insert(name.to_string(), val_str.to_string());
                            }
                        }
                        let body = response.body().clone();
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status,
                            error: None,
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status,
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag.clone().unwrap_or_else(|| url_str.clone());
                        let _ = tx.send(Metric::Request {
                            name: format!(
                                "{}{}",
                                crate::bridge::group::get_current_group_prefix(),
                                metric_name
                            ),
                            timings,
                            status: 0,
                            error: Some(e.to_string()),
                            tags: tags.clone(),
                        });
                        Ok(HttpResponse {
                            status: 0,
                            body: Bytes::from(e.to_string()),
                            headers: HashMap::new(),
                            timings,
                            proto: "h1".to_string(),
                        })
                    }
                }
            },
        ),
    )?;

    // http.formEncode(obj) - Encode object as application/x-www-form-urlencoded
    http.set(
        "formEncode",
        Function::new(ctx.clone(), move |obj: Object| -> Result<String> {
            let mut pairs: Vec<String> = Vec::new();
            for key in obj.keys::<String>() {
                if let Ok(k) = key {
                    let encoded_key = urlencoding::encode(&k);
                    if let Ok(v) = obj.get::<_, String>(&k) {
                        pairs.push(format!("{}={}", encoded_key, urlencoding::encode(&v)));
                    } else if let Ok(v) = obj.get::<_, i64>(&k) {
                        pairs.push(format!("{}={}", encoded_key, v));
                    } else if let Ok(v) = obj.get::<_, f64>(&k) {
                        pairs.push(format!("{}={}", encoded_key, v));
                    } else if let Ok(v) = obj.get::<_, bool>(&k) {
                        pairs.push(format!("{}={}", encoded_key, v));
                    }
                }
            }
            Ok(pairs.join("&"))
        }),
    )?;

    // http.url(base, params?) - Build URL with query parameters
    http.set(
        "url",
        Function::new(
            ctx.clone(),
            move |base_url: String, params: Option<Object>| -> Result<String> {
                if let Some(p) = params {
                    let mut url = Url::parse(&base_url)
                        .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;
                    for key in p.keys::<String>() {
                        if let Ok(k) = key {
                            if let Ok(v) = p.get::<_, String>(&k) {
                                url.query_pairs_mut().append_pair(&k, &v);
                            } else if let Ok(v) = p.get::<_, i64>(&k) {
                                url.query_pairs_mut().append_pair(&k, &v.to_string());
                            } else if let Ok(v) = p.get::<_, f64>(&k) {
                                url.query_pairs_mut().append_pair(&k, &v.to_string());
                            } else if let Ok(v) = p.get::<_, bool>(&k) {
                                url.query_pairs_mut().append_pair(&k, &v.to_string());
                            }
                        }
                    }
                    Ok(url.to_string())
                } else {
                    Ok(base_url)
                }
            },
        ),
    )?;

    // http.basicAuth(username, password) - Generate Basic auth header value
    http.set(
        "basicAuth",
        Function::new(
            ctx.clone(),
            move |username: String, password: String| -> String {
                use base64::{engine::general_purpose::STANDARD, Engine as _};
                let credentials = format!("{}:{}", username, password);
                format!("Basic {}", STANDARD.encode(credentials.as_bytes()))
            },
        ),
    )?;

    // http.bearerToken(token) - Generate Bearer auth header value
    http.set(
        "bearerToken",
        Function::new(ctx.clone(), move |token: String| -> String {
            format!("Bearer {}", token)
        }),
    )?;

    // http.setDefaults(opts) - Set global request defaults
    let defaults_set = defaults.clone();
    http.set(
        "setDefaults",
        Function::new(ctx.clone(), move |opts: Object| -> Result<()> {
            let mut d = defaults_set.borrow_mut();

            // Parse timeout
            if let Ok(timeout_str) = opts.get::<_, String>("timeout") {
                d.timeout = parse_duration_str(&timeout_str);
            } else if let Ok(timeout_ms) = opts.get::<_, u64>("timeout") {
                d.timeout = Some(Duration::from_millis(timeout_ms));
            }

            // Parse headers
            if let Ok(headers) = opts.get::<_, Object>("headers") {
                for key in headers.keys::<String>() {
                    if let Ok(k) = key {
                        if let Ok(v) = headers.get::<_, String>(&k) {
                            d.headers.insert(k, v);
                        }
                    }
                }
            }

            Ok(())
        }),
    )?;

    // http.batch(requests) - Execute multiple requests in parallel
    // requests: Array of { method, url, body?, headers?, name?, tags? }
    let c_batch = client.clone();
    let tx_batch = tx.clone();
    let rt_batch = runtime.clone();
    let sink_batch = global_response_sink;

    http.set("batch", Function::new(ctx.clone(), move |requests: rquickjs::Array<'_>| -> Result<Vec<HttpResponse>> {
        let client = c_batch.clone();
        let tx = tx_batch.clone();
        let runtime = rt_batch.clone();
        let response_sink = sink_batch;

        // Collect request specs
        let mut request_specs: Vec<(String, String, Option<String>, Option<HashMap<String, String>>, Option<String>, HashMap<String, String>, Option<Duration>)> = Vec::new();

        for item in requests.iter::<Object>() {
            let obj = item?;
            let method: String = obj.get("method").unwrap_or_else(|_| "GET".to_string());
            let url: String = obj.get("url").map_err(|_| rquickjs::Error::new_from_js("url is required", "ValueError"))?;
            let body: Option<String> = obj.get("body").ok();
            let headers: Option<HashMap<String, String>> = obj.get("headers").ok();
            let name: Option<String> = obj.get("name").ok();
            let tags: HashMap<String, String> = obj.get("tags").unwrap_or_default();
            let mut timeout_duration: Option<Duration> = Some(Duration::from_secs(60));
            if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                timeout_duration = parse_duration_str(&timeout_str);
            } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                timeout_duration = Some(Duration::from_millis(timeout_ms));
            }
            request_specs.push((method, url, body, headers, name, tags, timeout_duration));
        }

        // Execute all requests in parallel
        let results: Vec<HttpResponse> = runtime.block_on(async {
            let futures: Vec<_> = request_specs.into_iter().map(|(method_str, url_str, body, headers_map, name_opt, tags, timeout_duration)| {
                let client = client.clone();
                let tx = tx.clone();
                async move {
                    let url_parsed = match Url::parse(&url_str) {
                        Ok(u) => u,
                        Err(_) => {
                            return HttpResponse {
                                status: 0,
                                body: Bytes::from("Invalid URL"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                            };
                        }
                    };
                    let method = Method::from_bytes(method_str.to_uppercase().as_bytes()).unwrap_or(Method::GET);

                    let uri = match url_to_uri(&url_parsed) {
                        Some(u) => u,
                        None => {
                            return HttpResponse {
                                status: 0,
                                body: Bytes::from("Failed to convert URL to URI"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                            };
                        }
                    };

                    let mut req_builder = Request::builder()
                        .method(method)
                        .uri(uri);

                    if let Some(h) = &headers_map {
                        for (k, v) in h {
                            req_builder = req_builder.header(k, v);
                        }
                    }

                    let final_body = body.unwrap_or_default();
                    let req = match req_builder.body(final_body) {
                        Ok(r) => r,
                        Err(_) => {
                            return HttpResponse {
                                status: 0,
                                body: Bytes::from("Failed to build request"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                            };
                        }
                    };

                    let start = Instant::now();
                    let request_future = client.request(req, response_sink);

                    let res_result = if let Some(timeout) = timeout_duration {
                        match tokio::time::timeout(timeout, request_future).await {
                            Ok(result) => result,
                            Err(_) => {
                                let duration = start.elapsed();
                                let timings = RequestTimings { duration, ..Default::default() };
                                let metric_name = name_opt.clone().unwrap_or_else(|| url_str.clone());
                                let _ = tx.send(Metric::Request {
                                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                    timings,
                                    status: 0,
                                    error: Some("request timeout".to_string()),
                                    tags: tags.clone(),
                                });
                                return HttpResponse {
                                    status: 0,
                                    body: Bytes::from("request timeout"),
                                    headers: HashMap::new(),
                                    timings,
                                    proto: "h1".to_string(),
                                };
                            }
                        }
                    } else {
                        request_future.await
                    };

                    match res_result {
                        Ok((response, timings)) => {
                            let status = response.status().as_u16();
                            let proto = version_to_proto(response.version());
                            let mut resp_headers: HashMap<String, String> = HashMap::with_capacity(response.headers().len());
                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    resp_headers.insert(name.to_string(), val_str.to_string());
                                }
                            }
                            let body = response.body().clone();
                            let metric_name = name_opt.clone().unwrap_or_else(|| url_str.clone());
                            let _ = tx.send(Metric::Request {
                                name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                timings,
                                status,
                                error: None,
                                tags: tags.clone(),
                            });
                            HttpResponse { status, body, headers: resp_headers, timings, proto }
                        },
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings { duration, ..Default::default() };
                            let metric_name = name_opt.clone().unwrap_or_else(|| url_str.clone());
                            let _ = tx.send(Metric::Request {
                                name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                timings,
                                status: 0,
                                error: Some(e.to_string()),
                                tags: tags.clone(),
                            });
                            HttpResponse { status: 0, body: Bytes::from(e.to_string()), headers: HashMap::new(), timings, proto: "h1".to_string() }
                        }
                    }
                }
            }).collect();

            futures::future::join_all(futures).await
        });

        Ok(results)
    }))?;

    // http.file(path, filename?, contentType?)
    // Returns a JSON string that http.post can parse to detect multipart
    http.set(
        "file",
        Function::new(
            ctx.clone(),
            move |path: String,
                  filename: Option<String>,
                  content_type: Option<String>|
                  -> Result<String> {
                let file_path = PathBuf::from(&path);
                let data = std::fs::read(&file_path)
                    .map_err(|_| rquickjs::Error::new_from_js("IOError", "Failed to read file"))?;

                let fname = filename.unwrap_or_else(|| {
                    file_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".to_string())
                });

                let ctype = content_type.unwrap_or_else(|| "application/octet-stream".to_string());

                // Encode data as base64 and return as JSON
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&data);

                let json = serde_json::json!({
                    "__fusillade_file": true,
                    "data": b64,
                    "filename": fname,
                    "contentType": ctype
                });

                Ok(json.to_string())
            },
        ),
    )?;

    ctx.globals().set("http", http)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_to_proto_http1() {
        assert_eq!(version_to_proto(http::Version::HTTP_09), "h1");
        assert_eq!(version_to_proto(http::Version::HTTP_10), "h1");
        assert_eq!(version_to_proto(http::Version::HTTP_11), "h1");
    }

    #[test]
    fn test_version_to_proto_http2() {
        assert_eq!(version_to_proto(http::Version::HTTP_2), "h2");
    }

    #[test]
    fn test_version_to_proto_http3() {
        assert_eq!(version_to_proto(http::Version::HTTP_3), "h3");
    }

    #[test]
    fn test_http_response_default_proto() {
        let response = HttpResponse {
            status: 200,
            body: Bytes::from("test"),
            headers: HashMap::new(),
            timings: RequestTimings::default(),
            proto: "h1".to_string(),
        };
        assert_eq!(response.proto, "h1");
    }

    #[test]
    fn test_http_response_h2_proto() {
        let response = HttpResponse {
            status: 200,
            body: Bytes::from("test"),
            headers: HashMap::new(),
            timings: RequestTimings::default(),
            proto: "h2".to_string(),
        };
        assert_eq!(response.proto, "h2");
    }

    #[test]
    fn test_parse_duration_str_seconds() {
        assert_eq!(parse_duration_str("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration_str("1s"), Some(Duration::from_secs(1)));
    }

    #[test]
    fn test_parse_duration_str_milliseconds() {
        assert_eq!(
            parse_duration_str("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_duration_str("100ms"),
            Some(Duration::from_millis(100))
        );
    }

    #[test]
    fn test_parse_duration_str_minutes() {
        assert_eq!(parse_duration_str("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration_str("1m"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_parse_duration_str_raw_number() {
        assert_eq!(
            parse_duration_str("1000"),
            Some(Duration::from_millis(1000))
        );
    }

    #[test]
    fn test_url_to_uri_simple() {
        let url = Url::parse("https://api.example.com/users").unwrap();
        let uri = url_to_uri(&url).unwrap();
        assert_eq!(uri.scheme_str(), Some("https"));
        assert_eq!(uri.host(), Some("api.example.com"));
        assert_eq!(uri.path(), "/users");
    }

    #[test]
    fn test_url_to_uri_with_port() {
        let url = Url::parse("https://api.example.com:8080/users").unwrap();
        let uri = url_to_uri(&url).unwrap();
        assert_eq!(uri.port_u16(), Some(8080));
    }

    #[test]
    fn test_url_to_uri_with_query() {
        let url = Url::parse("https://api.example.com/search?q=test&page=1").unwrap();
        let uri = url_to_uri(&url).unwrap();
        assert_eq!(uri.query(), Some("q=test&page=1"));
    }
}
