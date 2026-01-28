use crate::engine::http_client::HttpClient;
use crate::stats::{Metric, RequestTimings};
use crate::utils::parse_duration_str;
use cookie::{time::OffsetDateTime, Cookie, SameSite};
use cookie_store::CookieStore;
use crossbeam_channel::Sender;
use http::{Method, Request, Uri};
use hyper::body::Bytes;
use rand::Rng;
use rquickjs::{Ctx, Function, IntoJs, Object, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::runtime::Runtime;
use url::Url;

/// Call all beforeRequest hooks with the request object (via JS shim)
fn call_before_request_hooks<'js>(ctx: &Ctx<'js>, req_obj: &Object<'js>) {
    if let Ok(call_hooks_fn) = ctx
        .globals()
        .get::<_, Function>("__http_callBeforeRequestHooks")
    {
        let _ = call_hooks_fn.call::<_, ()>((req_obj.clone(),));
    }
}

/// Call all afterResponse hooks with the response object (via JS shim)
fn call_after_response_hooks<'js>(ctx: &Ctx<'js>, res_obj: &Object<'js>) {
    if let Ok(call_hooks_fn) = ctx
        .globals()
        .get::<_, Function>("__http_callAfterResponseHooks")
    {
        let _ = call_hooks_fn.call::<_, ()>((res_obj.clone(),));
    }
}

/// Check if a path segment looks like an ID (numeric, UUID, hex, etc.)
fn is_id_segment(segment: &str) -> bool {
    // Pure numeric
    if segment.chars().all(|c| c.is_ascii_digit()) && !segment.is_empty() {
        return true;
    }

    // UUID pattern: 8-4-4-4-12 hex chars with dashes
    // e.g., 550e8400-e29b-41d4-a716-446655440000
    if segment.len() == 36 && segment.chars().filter(|c| *c == '-').count() == 4 {
        let parts: Vec<&str> = segment.split('-').collect();
        if parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
            && parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
        {
            return true;
        }
    }

    // UUID without dashes (32 hex chars)
    if segment.len() == 32 && segment.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }

    // Short IDs that are mostly alphanumeric with mixed case/numbers
    // e.g., "abc123", "X7yZ9" - but not common words
    if segment.len() >= 4 && segment.len() <= 24 {
        let has_digit = segment.chars().any(|c| c.is_ascii_digit());
        let all_alnum = segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if has_digit && all_alnum {
            // Likely an ID if it has digits mixed with letters
            return true;
        }
    }

    false
}

/// Normalize a URL path for metric naming by replacing variable segments with :id
fn normalize_url_for_metrics(url: &str) -> String {
    // Parse URL
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return url.to_string(),
    };

    let host = parsed.host_str().unwrap_or("unknown");
    let path = parsed.path();

    // Build host with port if non-standard
    let host_with_port = if let Some(port) = parsed.port() {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    // Replace segments that look like IDs with :id
    let normalized_path = path
        .split('/')
        .map(|segment| {
            if segment.is_empty() {
                segment.to_string()
            } else if is_id_segment(segment) {
                ":id".to_string()
            } else {
                segment.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    format!("{}{}", host_with_port, normalized_path)
}

/// JavaScript-exposed CookieJar class for manual cookie manipulation
/// Similar to k6's cookie jar API
#[derive(Clone)]
pub struct JsCookieJar {
    store: Rc<RefCell<CookieStore>>,
}

impl JsCookieJar {
    /// Set a cookie for a URL
    pub fn set<'js>(
        &self,
        url: String,
        name: String,
        value: String,
        opts: Option<Object<'js>>,
    ) -> Result<()> {
        let url_parsed = Url::parse(&url)
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

        let mut cookie = Cookie::new(name.clone(), value);

        if let Some(opts) = opts {
            // Domain
            if let Ok(domain) = opts.get::<_, String>("domain") {
                cookie.set_domain(domain);
            }

            // Path
            if let Ok(path) = opts.get::<_, String>("path") {
                cookie.set_path(path);
            }

            // Secure
            if let Ok(secure) = opts.get::<_, bool>("secure") {
                cookie.set_secure(secure);
            }

            // HttpOnly
            if let Ok(http_only) = opts.get::<_, bool>("httpOnly") {
                cookie.set_http_only(http_only);
            }

            // SameSite
            if let Ok(same_site) = opts.get::<_, String>("sameSite") {
                let ss = match same_site.to_lowercase().as_str() {
                    "strict" => SameSite::Strict,
                    "lax" => SameSite::Lax,
                    "none" => SameSite::None,
                    _ => SameSite::Lax,
                };
                cookie.set_same_site(ss);
            }

            // Expires (Unix timestamp in seconds)
            if let Ok(expires) = opts.get::<_, i64>("expires") {
                if let Ok(dt) = OffsetDateTime::from_unix_timestamp(expires) {
                    cookie.set_expires(dt);
                }
            }

            // MaxAge (in seconds)
            if let Ok(max_age) = opts.get::<_, i64>("maxAge") {
                cookie.set_max_age(cookie::time::Duration::seconds(max_age));
            }
        }

        let mut store = self.store.borrow_mut();
        let _ = store.insert_raw(&cookie, &url_parsed);
        Ok(())
    }

    /// Get a cookie value by name for a URL
    pub fn get(&self, url: String, name: String) -> Result<Option<String>> {
        let url_parsed = Url::parse(&url)
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

        let store = self.store.borrow();
        for cookie in store.matches(&url_parsed) {
            if cookie.name() == name {
                return Ok(Some(cookie.value().to_string()));
            }
        }
        Ok(None)
    }

    /// Get all cookies for a URL as a HashMap
    pub fn cookies_for_url_map(&self, url: String) -> Result<HashMap<String, String>> {
        let url_parsed = Url::parse(&url)
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

        let mut cookies = HashMap::new();
        let store = self.store.borrow();
        for cookie in store.matches(&url_parsed) {
            cookies.insert(cookie.name().to_string(), cookie.value().to_string());
        }
        Ok(cookies)
    }

    /// Clear all cookies
    pub fn clear(&self) {
        *self.store.borrow_mut() = CookieStore::default();
    }

    /// Delete a specific cookie by setting its expiry to the past
    pub fn delete(&self, url: String, name: String) -> Result<()> {
        let url_parsed = Url::parse(&url)
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URL", "UrlError"))?;

        // To delete a cookie, insert one with the same name but expired
        let mut cookie = Cookie::new(name, "");
        // Set expiry to Unix epoch (1970-01-01)
        cookie.set_expires(OffsetDateTime::UNIX_EPOCH);
        cookie.set_max_age(cookie::time::Duration::ZERO);

        let mut store = self.store.borrow_mut();
        let _ = store.insert_raw(&cookie, &url_parsed);
        Ok(())
    }
}

/// Global HTTP request defaults that can be set via http.setDefaults()
#[derive(Default, Clone)]
struct HttpDefaults {
    timeout: Option<Duration>,
    headers: HashMap<String, String>,
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

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub status_text: String,
    pub body: Bytes,
    pub headers: HashMap<String, String>,
    pub timings: RequestTimings,
    pub proto: String,
    /// Raw Set-Cookie header values for parsing into cookies object
    pub set_cookie_headers: Vec<String>,
    /// Error type for failed requests (e.g., "TIMEOUT", "DNS", "TLS", "CONNECT", "RESET")
    pub error: Option<String>,
    /// Error code for failed requests (e.g., "ETIMEDOUT", "ENOTFOUND", "ECERT", "ECONNREFUSED")
    pub error_code: Option<String>,
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

impl<'js> IntoJs<'js> for HttpResponse {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("status", self.status)?;
        obj.set("statusText", self.status_text)?;
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

        // Add cookies property - parse Set-Cookie headers
        // Format: { cookieName: { value, domain, path, expires, httpOnly, secure, sameSite }, ... }
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

            // Custom retry condition (retryOn function)
            let retry_on_fn: Option<Function> = opts.get("retryOn").ok();
            // Custom retry delay function (retryDelayFn function)
            let retry_delay_fn: Option<Function> = opts.get("retryDelayFn").ok();

            // Redirect configuration
            let follow_redirects: bool = opts.get("followRedirects").unwrap_or(true);
            let max_redirects: u32 = opts.get("maxRedirects").unwrap_or(5);

            // Call beforeRequest hooks
            {
                let req_obj = Object::new(ctx.clone())?;
                req_obj.set("method", method_str.clone())?;
                req_obj.set("url", url_str.clone())?;
                if let Some(ref b) = body {
                    req_obj.set("body", b.clone())?;
                }
                if let Some(ref h) = headers_map {
                    let headers_obj = Object::new(ctx.clone())?;
                    for (k, v) in h {
                        headers_obj.set(k.as_str(), v.as_str())?;
                    }
                    req_obj.set("headers", headers_obj)?;
                }
                call_before_request_hooks(ctx, &req_obj);
            }

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
                                let name_lower = name_str.to_lowercase();
                                if name_lower == "set-cookie" {
                                    resp_headers
                                        .entry(name_str.clone())
                                        .and_modify(|v| {
                                            v.push('\n');
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                    set_cookie_headers.push(val_str.to_string());
                                } else {
                                    resp_headers
                                        .entry(name_str.clone())
                                        .and_modify(|v| {
                                            v.push_str(", ");
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                }
                                if name_lower == "location" {
                                    location_header = Some(val_str.to_string());
                                }
                            }
                        }

                        // Body is already read in client.request and inside response
                        // When response_sink is enabled, body will be empty (Bytes::new())
                        let body = response.body().clone();

                        let metric_name = name_opt
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            for cookie_val in &set_cookie_headers {
                                let _ = store.parse(cookie_val, &url_parsed);
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

                        // Check custom retry condition (retryOn)
                        if let Some(ref retry_fn) = retry_on_fn {
                            if retry_count < max_retries {
                                // Create response object for retry check
                                let res_obj = Object::new(ctx.clone())?;
                                res_obj.set("status", status)?;
                                res_obj.set("body", String::from_utf8_lossy(&body).to_string())?;
                                let headers_obj = Object::new(ctx.clone())?;
                                for (k, v) in &resp_headers {
                                    headers_obj.set(k.as_str(), v.as_str())?;
                                }
                                res_obj.set("headers", headers_obj)?;

                                if let Ok(should_retry) =
                                    retry_fn.call::<_, rquickjs::Value>((res_obj,))
                                {
                                    if should_retry.as_bool().unwrap_or(false) {
                                        retry_count += 1;
                                        // Calculate delay using retryDelayFn if provided
                                        let delay_ms = if let Some(ref delay_fn) = retry_delay_fn {
                                            delay_fn.call::<_, u64>((retry_count,)).unwrap_or(
                                                retry_delay_ms * (1 << (retry_count - 1).min(5)),
                                            )
                                        } else {
                                            retry_delay_ms * (1 << (retry_count - 1).min(5))
                                        };
                                        runtime.block_on(async {
                                            tokio::time::sleep(Duration::from_millis(delay_ms))
                                                .await;
                                        });
                                        redirects = 0;
                                        continue;
                                    }
                                }
                            }
                        }

                        // Call afterResponse hooks
                        {
                            let res_obj = Object::new(ctx.clone())?;
                            res_obj.set("status", status)?;
                            res_obj.set("body", String::from_utf8_lossy(&body).to_string())?;
                            let headers_obj = Object::new(ctx.clone())?;
                            for (k, v) in &resp_headers {
                                headers_obj.set(k.as_str(), v.as_str())?;
                            }
                            res_obj.set("headers", headers_obj)?;
                            res_obj.set("proto", proto.clone())?;
                            call_after_response_hooks(ctx, &res_obj);
                        }

                        return Ok(HttpResponse {
                            status,
                            status_text: status_text_for_code(status),
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                            set_cookie_headers,
                            error: None,
                            error_code: None,
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
                        let metric_name = name_opt
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));

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
                        let (error_type, error_code) = categorize_error(&e.to_string());
                        return Ok(HttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: Bytes::from(e.to_string()),
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
        }),
    )?;

    // Helper GET
    let c_get = client.clone();
    let tx_get = tx.clone();
    let cs_get = cookie_store.clone();
    let rt_get = runtime.clone();
    let sink_get = global_response_sink;
    let defaults_get = defaults.clone();

    http.set(
        "get",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_get.borrow();

                let mut name_tag = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                let mut max_retries: u32 = 0;
                let mut retry_delay_ms: u64 = 100;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                        max_retries = obj.get("retry").unwrap_or(0);
                        retry_delay_ms = obj.get("retryDelay").unwrap_or(100);
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                let mut retry_count: u32 = 0;

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
                        req_builder = req_builder.header("Cookie", cookie_header_val);
                    }
                    // Apply merged headers
                    if let Some(ref h) = headers_map {
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
                            let mut set_cookie_headers: Vec<String> = Vec::new();
                            let mut location_header = None;

                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    let name_lower = name_str.to_lowercase();
                                    if name_lower == "set-cookie" {
                                        resp_headers
                                            .entry(name_str.clone())
                                            .and_modify(|v| {
                                                v.push('\n');
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                        set_cookie_headers.push(val_str.to_string());
                                    } else {
                                        resp_headers
                                            .entry(name_str.clone())
                                            .and_modify(|v| {
                                                v.push_str(", ");
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                    }
                                    if name_lower == "location" {
                                        location_header = Some(val_str.to_string());
                                    }
                                }
                            }
                            let body = response.body().clone();

                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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
                                for cookie_val in &set_cookie_headers {
                                    if store.parse(cookie_val, &url_parsed).is_ok() {}
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
                                status_text: status_text_for_code(status),
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                                set_cookie_headers,
                                error: None,
                                error_code: None,
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
                                redirects = 0;
                                continue;
                            }

                            let duration = start.elapsed();
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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
                            let (error_type, error_code) = categorize_error(&e.to_string());
                            return Ok(HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from(e.to_string()),
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
            },
        ),
    )?;

    // Helper POST with FormData detection
    let c_post = client.clone();
    let tx_post = tx.clone();
    let cs_post = cookie_store.clone();
    let rt_post = runtime.clone();
    let sink_post = global_response_sink;
    let defaults_post = defaults.clone();

    http.set(
        "post",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_post.borrow();

                let mut name_tag = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                let mut max_retries: u32 = 0;
                let mut retry_delay_ms: u64 = 100;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                        max_retries = obj.get("retry").unwrap_or(0);
                        retry_delay_ms = obj.get("retryDelay").unwrap_or(100);
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                let mut retry_count: u32 = 0;

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
                                            let name_str = name.to_string();
                                            let name_lower = name_str.to_lowercase();
                                            if name_lower == "set-cookie" {
                                                resp_headers
                                                    .entry(name_str)
                                                    .and_modify(|v| {
                                                        v.push('\n');
                                                        v.push_str(val_str);
                                                    })
                                                    .or_insert_with(|| val_str.to_string());
                                            } else {
                                                resp_headers
                                                    .entry(name_str)
                                                    .and_modify(|v| {
                                                        v.push_str(", ");
                                                        v.push_str(val_str);
                                                    })
                                                    .or_insert_with(|| val_str.to_string());
                                            }
                                        }
                                    }
                                    let body_bytes = response.bytes().unwrap_or_default();

                                    let timings = RequestTimings {
                                        duration,
                                        ..Default::default()
                                    };
                                    let metric_name = name_tag
                                        .clone()
                                        .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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

                                    // For multipart, we don't have easy access to set-cookie headers from reqwest
                                    // Extract from resp_headers if present
                                    let mut multipart_set_cookie: Vec<String> = Vec::new();
                                    if let Some(sc) = resp_headers.get("set-cookie") {
                                        for line in sc.lines() {
                                            multipart_set_cookie.push(line.to_string());
                                        }
                                    }
                                    return Ok(HttpResponse {
                                        status,
                                        status_text: status_text_for_code(status),
                                        body: Bytes::from(body_bytes.to_vec()),
                                        headers: resp_headers,
                                        timings,
                                        proto,
                                        set_cookie_headers: multipart_set_cookie,
                                        error: None,
                                        error_code: None,
                                    });
                                }
                                Err(e) => {
                                    let timings = RequestTimings {
                                        duration,
                                        ..Default::default()
                                    };
                                    let metric_name = name_tag
                                        .clone()
                                        .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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
                                    let (error_type, error_code) = categorize_error(&e.to_string());
                                    return Ok(HttpResponse {
                                        status: 0,
                                        status_text: status_text_for_code(0),
                                        body: Bytes::from(e.to_string()),
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
                    }
                }

                // Standard POST (non-multipart)
                let original_body = body.clone();
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
                                    let name_lower = name_str.to_lowercase();
                                    if name_lower == "set-cookie" {
                                        resp_headers
                                            .entry(name_str.clone())
                                            .and_modify(|v| {
                                                v.push('\n');
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                        set_cookie_headers.push(val_str.to_string());
                                    } else {
                                        resp_headers
                                            .entry(name_str.clone())
                                            .and_modify(|v| {
                                                v.push_str(", ");
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                    }
                                    if name_lower == "location" {
                                        location_header = Some(val_str.to_string());
                                    }
                                }
                            }

                            let body = response.body().clone();

                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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
                                for cookie_val in &set_cookie_headers {
                                    let _ = store.parse(cookie_val, &url_parsed);
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
                                status_text: status_text_for_code(status),
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                                set_cookie_headers,
                                error: None,
                                error_code: None,
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
                                redirects = 0;
                                current_method = Method::POST;
                                current_body = Some(original_body.clone());
                                continue;
                            }

                            let duration = start.elapsed();
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&current_url));
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
                            let (error_type, error_code) = categorize_error(&e.to_string());
                            return Ok(HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from(e.to_string()),
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
            },
        ),
    )?;

    // Helper PUT
    let c_put = client.clone();
    let tx_put = tx.clone();
    let cs_put = cookie_store.clone();
    let rt_put = runtime.clone();
    let sink_put = global_response_sink;
    let defaults_put = defaults.clone();

    http.set(
        "put",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_put.borrow();

                let mut name_tag = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                let mut max_retries: u32 = 0;
                let mut retry_delay_ms: u64 = 100;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                        max_retries = obj.get("retry").unwrap_or(0);
                        retry_delay_ms = obj.get("retryDelay").unwrap_or(100);
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                let mut retry_count: u32 = 0;

                loop {
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
                        .body(body.clone())
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
                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    let name_lower = name_str.to_lowercase();
                                    if name_lower == "set-cookie" {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push('\n');
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                        set_cookie_headers.push(val_str.to_string());
                                    } else {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push_str(", ");
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                    }
                                }
                            }
                            let body = response.body().clone();
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                                status_text: status_text_for_code(status),
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                                set_cookie_headers,
                                error: None,
                                error_code: None,
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
                                continue;
                            }

                            let timings = RequestTimings {
                                duration: start.elapsed(),
                                ..Default::default()
                            };
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            let (error_type, error_code) = categorize_error(&e.to_string());
                            return Ok(HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from(e.to_string()),
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
            },
        ),
    )?;

    // Helper DELETE
    let c_del = client.clone();
    let tx_del = tx.clone();
    let cs_del = cookie_store.clone();
    let rt_del = runtime.clone();
    let sink_del = global_response_sink;
    let defaults_del = defaults.clone();

    http.set(
        "del",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_del.borrow();

                let mut name_tag: Option<String> = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                let mut max_retries: u32 = 0;
                let mut retry_delay_ms: u64 = 100;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                        max_retries = obj.get("retry").unwrap_or(0);
                        retry_delay_ms = obj.get("retryDelay").unwrap_or(100);
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                let mut retry_count: u32 = 0;

                loop {
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
                            let mut set_cookie_headers: Vec<String> = Vec::new();
                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    let name_lower = name_str.to_lowercase();
                                    if name_lower == "set-cookie" {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push('\n');
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                        set_cookie_headers.push(val_str.to_string());
                                    } else {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push_str(", ");
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                    }
                                }
                            }
                            let body = response.body().clone();
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                                status_text: status_text_for_code(status),
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                                set_cookie_headers,
                                error: None,
                                error_code: None,
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
                                continue;
                            }

                            let timings = RequestTimings {
                                duration: start.elapsed(),
                                ..Default::default()
                            };
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            let (error_type, error_code) = categorize_error(&e.to_string());
                            return Ok(HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from(e.to_string()),
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
            },
        ),
    )?;

    // Helper PATCH
    let c_patch = client.clone();
    let tx_patch = tx.clone();
    let cs_patch = cookie_store.clone();
    let rt_patch = runtime.clone();
    let sink_patch = global_response_sink;
    let defaults_patch = defaults.clone();

    http.set(
        "patch",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  body: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_patch.borrow();

                let mut name_tag: Option<String> = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                let mut max_retries: u32 = 0;
                let mut retry_delay_ms: u64 = 100;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                        max_retries = obj.get("retry").unwrap_or(0);
                        retry_delay_ms = obj.get("retryDelay").unwrap_or(100);
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                let mut retry_count: u32 = 0;

                loop {
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
                        .body(body.clone())
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
                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                    let name_lower = name_str.to_lowercase();
                                    if name_lower == "set-cookie" {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push('\n');
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                        set_cookie_headers.push(val_str.to_string());
                                    } else {
                                        resp_headers
                                            .entry(name_str)
                                            .and_modify(|v| {
                                                v.push_str(", ");
                                                v.push_str(val_str);
                                            })
                                            .or_insert_with(|| val_str.to_string());
                                    }
                                }
                            }
                            let body = response.body().clone();
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                                status_text: status_text_for_code(status),
                                body,
                                headers: resp_headers,
                                timings,
                                proto,
                                set_cookie_headers,
                                error: None,
                                error_code: None,
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
                                continue;
                            }

                            let timings = RequestTimings {
                                duration: start.elapsed(),
                                ..Default::default()
                            };
                            let metric_name = name_tag
                                .clone()
                                .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            let (error_type, error_code) = categorize_error(&e.to_string());
                            return Ok(HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from(e.to_string()),
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
            },
        ),
    )?;

    // Helper HEAD
    let c_head = client.clone();
    let tx_head = tx.clone();
    let cs_head = cookie_store.clone();
    let rt_head = runtime.clone();
    let sink_head = global_response_sink;
    let defaults_head = defaults.clone();

    http.set(
        "head",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_head.borrow();

                let mut name_tag: Option<String> = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                // Apply merged headers
                if let Some(ref h) = headers_map {
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
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                let name_str = name.to_string();
                                let name_lower = name_str.to_lowercase();
                                if name_lower == "set-cookie" {
                                    resp_headers
                                        .entry(name_str)
                                        .and_modify(|v| {
                                            v.push('\n');
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                    set_cookie_headers.push(val_str.to_string());
                                } else {
                                    resp_headers
                                        .entry(name_str)
                                        .and_modify(|v| {
                                            v.push_str(", ");
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                }
                            }
                        }
                        let metric_name = name_tag
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            status_text: status_text_for_code(status),
                            body: Bytes::new(),
                            headers: resp_headers,
                            timings,
                            proto,
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                        let (error_type, error_code) = categorize_error(&e.to_string());
                        Ok(HttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: Bytes::from(e.to_string()),
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

    // Helper OPTIONS
    let c_opts = client.clone();
    let tx_opts = tx.clone();
    let cs_opts = cookie_store.clone();
    let rt_opts = runtime.clone();
    let sink_opts = global_response_sink;
    let defaults_opts = defaults.clone();

    http.set(
        "options",
        Function::new(
            ctx.clone(),
            move |url_str: String,
                  rest: rquickjs::function::Rest<rquickjs::Value>|
                  -> Result<HttpResponse> {
                // Borrow defaults
                let defs = defaults_opts.borrow();

                let mut name_tag: Option<String> = None;
                let mut request_headers: Option<HashMap<String, String>> = None;
                let mut timeout_duration: Option<Duration> = None;
                if let Some(arg) = rest.first() {
                    if let Some(obj) = arg.as_object() {
                        name_tag = obj.get::<_, String>("name").ok();
                        request_headers = obj.get("headers").ok();
                        if let Ok(timeout_str) = obj.get::<_, String>("timeout") {
                            timeout_duration = parse_duration_str(&timeout_str);
                        } else if let Ok(timeout_ms) = obj.get::<_, u64>("timeout") {
                            timeout_duration = Some(Duration::from_millis(timeout_ms));
                        }
                    }
                }

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

                // Use default timeout if not specified in request
                if timeout_duration.is_none() {
                    timeout_duration = defs.timeout.or(Some(Duration::from_secs(60)));
                }

                // Drop the borrow before the async block
                std::mem::drop(defs);

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
                // Apply merged headers
                if let Some(ref h) = headers_map {
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
                        for (name, val) in response.headers() {
                            if let Ok(val_str) = val.to_str() {
                                let name_str = name.to_string();
                                let name_lower = name_str.to_lowercase();
                                if name_lower == "set-cookie" {
                                    resp_headers
                                        .entry(name_str)
                                        .and_modify(|v| {
                                            v.push('\n');
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                    set_cookie_headers.push(val_str.to_string());
                                } else {
                                    resp_headers
                                        .entry(name_str)
                                        .and_modify(|v| {
                                            v.push_str(", ");
                                            v.push_str(val_str);
                                        })
                                        .or_insert_with(|| val_str.to_string());
                                }
                            }
                        }
                        let body = response.body().clone();
                        let metric_name = name_tag
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                            status_text: status_text_for_code(status),
                            body,
                            headers: resp_headers,
                            timings,
                            proto,
                            set_cookie_headers,
                            error: None,
                            error_code: None,
                        })
                    }
                    Err(e) => {
                        let timings = RequestTimings {
                            duration: start.elapsed(),
                            ..Default::default()
                        };
                        let metric_name = name_tag
                            .clone()
                            .unwrap_or_else(|| normalize_url_for_metrics(&url_str));
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
                        let (error_type, error_code) = categorize_error(&e.to_string());
                        Ok(HttpResponse {
                            status: 0,
                            status_text: status_text_for_code(0),
                            body: Bytes::from(e.to_string()),
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

    // http.formEncode(obj) - Encode object as application/x-www-form-urlencoded
    http.set(
        "formEncode",
        Function::new(ctx.clone(), move |obj: Object| -> Result<String> {
            let mut pairs: Vec<String> = Vec::new();
            for k in obj.keys::<String>().flatten() {
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
                    for k in p.keys::<String>().flatten() {
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
                for k in headers.keys::<String>().flatten() {
                    if let Ok(v) = headers.get::<_, String>(&k) {
                        d.headers.insert(k, v);
                    }
                }
            }

            Ok(())
        }),
    )?;

    // Initialize HTTP hooks infrastructure in JavaScript
    // This avoids Rust lifetime issues with storing JS functions
    ctx.eval::<(), _>(
        r#"
        globalThis.__http_hooks = globalThis.__http_hooks || {
            beforeRequest: [],
            afterResponse: []
        };
        globalThis.__http_callBeforeRequestHooks = function(req) {
            for (var i = 0; i < globalThis.__http_hooks.beforeRequest.length; i++) {
                try {
                    globalThis.__http_hooks.beforeRequest[i](req);
                } catch(e) {}
            }
        };
        globalThis.__http_callAfterResponseHooks = function(res) {
            for (var i = 0; i < globalThis.__http_hooks.afterResponse.length; i++) {
                try {
                    globalThis.__http_hooks.afterResponse[i](res);
                } catch(e) {}
            }
        };
    "#,
    )?;

    // http.addHook and http.clearHooks are defined purely in JS (see end of function)

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
        #[allow(clippy::type_complexity)]
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
                                status_text: status_text_for_code(0),
                                body: Bytes::from("Invalid URL"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some("NETWORK".to_string()),
                                error_code: Some("EINVAL".to_string()),
                            };
                        }
                    };
                    let method = Method::from_bytes(method_str.to_uppercase().as_bytes()).unwrap_or(Method::GET);

                    let uri = match url_to_uri(&url_parsed) {
                        Some(u) => u,
                        None => {
                            return HttpResponse {
                                status: 0,
                                status_text: status_text_for_code(0),
                                body: Bytes::from("Failed to convert URL to URI"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some("NETWORK".to_string()),
                                error_code: Some("EINVAL".to_string()),
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
                                status_text: status_text_for_code(0),
                                body: Bytes::from("Failed to build request"),
                                headers: HashMap::new(),
                                timings: RequestTimings::default(),
                                proto: "h1".to_string(),
                                set_cookie_headers: Vec::new(),
                                error: Some("NETWORK".to_string()),
                                error_code: Some("EINVAL".to_string()),
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
                                let metric_name = name_opt.clone().unwrap_or_else(|| normalize_url_for_metrics(&url_str));
                                let _ = tx.send(Metric::Request {
                                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                    timings,
                                    status: 0,
                                    error: Some("request timeout".to_string()),
                                    tags: tags.clone(),
                                });
                                return HttpResponse {
                                    status: 0,
                                    status_text: status_text_for_code(0),
                                    body: Bytes::from("request timeout"),
                                    headers: HashMap::new(),
                                    timings,
                                    proto: "h1".to_string(),
                                    set_cookie_headers: Vec::new(),
                                    error: Some("TIMEOUT".to_string()),
                                    error_code: Some("ETIMEDOUT".to_string()),
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
                            let mut set_cookie_headers: Vec<String> = Vec::new();
                            for (name, val) in response.headers() {
                                if let Ok(val_str) = val.to_str() {
                                    let name_str = name.to_string();
                                let name_lower = name_str.to_lowercase();
                                if name_lower == "set-cookie" {
                                    resp_headers.entry(name_str)
                                        .and_modify(|v| { v.push('\n'); v.push_str(val_str); })
                                        .or_insert_with(|| val_str.to_string());
                                    set_cookie_headers.push(val_str.to_string());
                                } else {
                                    resp_headers.entry(name_str)
                                        .and_modify(|v| { v.push_str(", "); v.push_str(val_str); })
                                        .or_insert_with(|| val_str.to_string());
                                }
                                }
                            }
                            let body = response.body().clone();
                            let metric_name = name_opt.clone().unwrap_or_else(|| normalize_url_for_metrics(&url_str));
                            let _ = tx.send(Metric::Request {
                                name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                timings,
                                status,
                                error: None,
                                tags: tags.clone(),
                            });
                            HttpResponse { status, status_text: status_text_for_code(status), body, headers: resp_headers, timings, proto, set_cookie_headers, error: None, error_code: None }
                        },
                        Err(e) => {
                            let duration = start.elapsed();
                            let timings = RequestTimings { duration, ..Default::default() };
                            let metric_name = name_opt.clone().unwrap_or_else(|| normalize_url_for_metrics(&url_str));
                            let (error_type, err_code) = categorize_error(&e.to_string());
                            let _ = tx.send(Metric::Request {
                                name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                                timings,
                                status: 0,
                                error: Some(e.to_string()),
                                tags: tags.clone(),
                            });
                            HttpResponse { status: 0, status_text: status_text_for_code(0), body: Bytes::from(e.to_string()), headers: HashMap::new(), timings, proto: "h1".to_string(), set_cookie_headers: Vec::new(), error: Some(error_type), error_code: Some(err_code) }
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

    // http.cookieJar() - Returns the cookie jar for manual cookie manipulation
    // We need to create the jar object upfront and return a reference to it each time
    let jar = JsCookieJar {
        store: cookie_store.clone(),
    };

    // Pre-create the jar object with all methods
    let jar_obj = Object::new(ctx.clone())?;

    // jar.set(url, name, value, opts?)
    let jar_set = jar.clone();
    jar_obj.set(
        "set",
        Function::new(
            ctx.clone(),
            move |url: String,
                  name: String,
                  value: String,
                  opts: Option<Object<'_>>|
                  -> Result<()> { jar_set.set(url, name, value, opts) },
        ),
    )?;

    // jar.get(url, name)
    let jar_get = jar.clone();
    jar_obj.set(
        "get",
        Function::new(
            ctx.clone(),
            move |url: String, name: String| -> Result<Option<String>> { jar_get.get(url, name) },
        ),
    )?;

    // jar.cookiesForUrl(url) - returns a HashMap
    let jar_cookies = jar.clone();
    jar_obj.set(
        "cookiesForUrl",
        Function::new(
            ctx.clone(),
            move |url: String| -> Result<HashMap<String, String>> {
                jar_cookies.cookies_for_url_map(url)
            },
        ),
    )?;

    // jar.clear()
    let jar_clear = jar.clone();
    jar_obj.set(
        "clear",
        Function::new(ctx.clone(), move || {
            jar_clear.clear();
        }),
    )?;

    // jar.delete(url, name)
    let jar_delete = jar;
    jar_obj.set(
        "delete",
        Function::new(
            ctx.clone(),
            move |url: String, name: String| -> Result<()> { jar_delete.delete(url, name) },
        ),
    )?;

    // http.cookieJar() returns the pre-created jar object
    http.set(
        "cookieJar",
        Function::new(ctx.clone(), move || -> Object<'_> { jar_obj.clone() }),
    )?;

    ctx.globals().set("http", http)?;

    // Register FormData class using JavaScript
    // This creates a proper FormData builder that works with http.post()
    ctx.eval::<(), _>(r#"
        globalThis.FormData = function() {
            this._fields = [];
            this._boundary = '----FusilladeBoundary' + Math.random().toString(16).substring(2);
        };

        FormData.prototype.append = function(name, value) {
            // Check if value is a file marker (from http.file())
            var isFile = false;
            var fileData = null;

            if (typeof value === 'string' && value.indexOf('"__fusillade_file":true') !== -1) {
                try {
                    fileData = JSON.parse(value);
                    isFile = true;
                } catch(e) {}
            }

            this._fields.push({
                name: name,
                value: isFile ? fileData : String(value),
                isFile: isFile
            });
        };

        FormData.prototype.body = function() {
            var body = '';
            for (var i = 0; i < this._fields.length; i++) {
                var field = this._fields[i];
                body += '--' + this._boundary + '\r\n';

                if (field.isFile && field.value) {
                    body += 'Content-Disposition: form-data; name="' + field.name + '"; filename="' + (field.value.filename || 'file') + '"\r\n';
                    body += 'Content-Type: ' + (field.value.contentType || 'application/octet-stream') + '\r\n\r\n';
                    // Note: For binary data, this will be lossy. Use the JSON-based approach for true binary.
                    body += atob(field.value.data || '');
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

        FormData.prototype.hasFiles = function() {
            return this._fields.some(function(f) { return f.isFile; });
        };

        // Add http.addHook and http.clearHooks methods
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
    "#)?;
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
            status_text: status_text_for_code(200),
            body: Bytes::from("test"),
            headers: HashMap::new(),
            timings: RequestTimings::default(),
            proto: "h1".to_string(),
            set_cookie_headers: Vec::new(),
            error: None,
            error_code: None,
        };
        assert_eq!(response.proto, "h1");
    }

    #[test]
    fn test_http_response_h2_proto() {
        let response = HttpResponse {
            status: 200,
            status_text: status_text_for_code(200),
            body: Bytes::from("test"),
            headers: HashMap::new(),
            timings: RequestTimings::default(),
            proto: "h2".to_string(),
            set_cookie_headers: Vec::new(),
            error: None,
            error_code: None,
        };
        assert_eq!(response.proto, "h2");
    }

    #[test]
    fn test_categorize_error_timeout() {
        let (error_type, error_code) = categorize_error("request timed out");
        assert_eq!(error_type, "TIMEOUT");
        assert_eq!(error_code, "ETIMEDOUT");
    }

    #[test]
    fn test_categorize_error_dns() {
        let (error_type, error_code) = categorize_error("failed to resolve DNS");
        assert_eq!(error_type, "DNS");
        assert_eq!(error_code, "ENOTFOUND");
    }

    #[test]
    fn test_categorize_error_connection_refused() {
        let (error_type, error_code) = categorize_error("connection refused");
        assert_eq!(error_type, "CONNECT");
        assert_eq!(error_code, "ECONNREFUSED");
    }

    #[test]
    fn test_categorize_error_tls() {
        let (error_type, error_code) = categorize_error("certificate error: invalid");
        assert_eq!(error_type, "TLS");
        assert_eq!(error_code, "ECERT");
    }

    #[test]
    fn test_categorize_error_reset() {
        let (error_type, error_code) = categorize_error("connection reset by peer");
        assert_eq!(error_type, "RESET");
        assert_eq!(error_code, "ECONNRESET");
    }

    #[test]
    fn test_categorize_error_unknown() {
        let (error_type, error_code) = categorize_error("some unknown error");
        assert_eq!(error_type, "NETWORK");
        assert_eq!(error_code, "ENETWORK");
    }

    // Note: parse_duration_str tests are in src/utils.rs

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

    // Tests for is_id_segment

    #[test]
    fn test_is_id_segment_numeric() {
        assert!(is_id_segment("123"));
        assert!(is_id_segment("0"));
        assert!(is_id_segment("999999"));
    }

    #[test]
    fn test_is_id_segment_uuid_with_dashes() {
        assert!(is_id_segment("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_id_segment("00000000-0000-0000-0000-000000000000"));
        assert!(is_id_segment("ffffffff-ffff-ffff-ffff-ffffffffffff"));
    }

    #[test]
    fn test_is_id_segment_uuid_without_dashes() {
        assert!(is_id_segment("550e8400e29b41d4a716446655440000"));
        assert!(is_id_segment("00000000000000000000000000000000"));
    }

    #[test]
    fn test_is_id_segment_alphanumeric_ids() {
        assert!(is_id_segment("abc123"));
        assert!(is_id_segment("X7yZ9"));
        assert!(is_id_segment("user_123"));
        assert!(is_id_segment("item-456"));
    }

    #[test]
    fn test_is_id_segment_not_ids() {
        // Common path segments that should NOT be treated as IDs
        assert!(!is_id_segment("users"));
        assert!(!is_id_segment("api"));
        assert!(!is_id_segment("items"));
        assert!(!is_id_segment("orders"));
        assert!(!is_id_segment("v1")); // Short, but commonly version prefixes
    }

    #[test]
    fn test_is_id_segment_empty() {
        assert!(!is_id_segment(""));
    }

    // Tests for normalize_url_for_metrics

    #[test]
    fn test_normalize_url_numeric_id() {
        assert_eq!(
            normalize_url_for_metrics("https://api.com/users/123"),
            "api.com/users/:id"
        );
    }

    #[test]
    fn test_normalize_url_multiple_ids() {
        assert_eq!(
            normalize_url_for_metrics("https://api.com/orders/abc-123-def/items/5"),
            "api.com/orders/:id/items/:id"
        );
    }

    #[test]
    fn test_normalize_url_uuid() {
        assert_eq!(
            normalize_url_for_metrics(
                "https://api.example.com/resources/550e8400-e29b-41d4-a716-446655440000"
            ),
            "api.example.com/resources/:id"
        );
    }

    #[test]
    fn test_normalize_url_no_ids() {
        assert_eq!(
            normalize_url_for_metrics("https://api.example.com/users/profile"),
            "api.example.com/users/profile"
        );
    }

    #[test]
    fn test_normalize_url_with_port() {
        assert_eq!(
            normalize_url_for_metrics("https://api.example.com:8080/users/123"),
            "api.example.com:8080/users/:id"
        );
    }

    #[test]
    fn test_normalize_url_preserves_query_stripped() {
        // Query params are not included in the normalized URL (just host + path)
        assert_eq!(
            normalize_url_for_metrics("https://api.example.com/search?q=test"),
            "api.example.com/search"
        );
    }

    #[test]
    fn test_normalize_url_invalid_url() {
        // Invalid URLs should be returned as-is
        assert_eq!(
            normalize_url_for_metrics("not-a-valid-url"),
            "not-a-valid-url"
        );
    }

    #[test]
    fn test_normalize_url_root_path() {
        assert_eq!(
            normalize_url_for_metrics("https://api.example.com/"),
            "api.example.com/"
        );
    }
}
