//! Synchronous HTTP bridge using ureq - bypasses Tokio for lower latency
//! This module provides a drop-in replacement for http.rs that uses blocking I/O
//! directly compatible with May green threads.

use rquickjs::{Ctx, Function, Object, Result, Value, IntoJs};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::stats::{Metric, RequestTimings};
use crossbeam_channel::Sender;
use std::borrow::Cow;

/// Sync HTTP response (matches HttpResponse from http.rs)
#[derive(Debug)]
pub struct SyncHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub timings: RequestTimings,
}

impl<'js> IntoJs<'js> for SyncHttpResponse {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("status", self.status)?;
        obj.set("proto", "h1")?;

        let body_str = String::from_utf8_lossy(&self.body);
        let body_string = body_str.to_string();
        obj.set("body", body_str.as_ref())?;

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
        obj.set("json", Function::new(ctx.clone(), move || -> Result<Value<'_>> {
            let json_global: Object = ctx_clone.globals().get("JSON")?;
            let parse: Function = json_global.get("parse")?;
            parse.call((body_string.clone(),))
        }))?;

        Ok(obj.into_value())
    }
}

// Thread-local ureq agent for connection pooling
thread_local! {
    static AGENT: ureq::Agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(60))
        .build();
}

/// Register synchronous HTTP functions using ureq (no Tokio overhead)
pub fn register_sync_http(ctx: &Ctx, tx: Sender<Metric>, response_sink: bool) -> Result<()> {
    let http = Object::new(ctx.clone())?;
    let global_response_sink = response_sink;

    // GET - most common, highly optimized
    let tx_get = tx.clone();
    let sink_get = global_response_sink;
    http.set("get", Function::new(ctx.clone(), move |url_str: String, rest: rquickjs::function::Rest<rquickjs::Value>| -> Result<SyncHttpResponse> {
        let name_tag: Option<String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get::<_, String>("name").ok());

        let tags: HashMap<String, String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get("tags").ok())
            .unwrap_or_default();

        let tx = tx_get.clone();
        let start = Instant::now();

        let result = AGENT.with(|agent| {
            agent.get(&url_str).call()
        });

        let waiting = start.elapsed();

        match result {
            Ok(response) => {
                let status = response.status();
                let mut headers = HashMap::new();
                for name in response.headers_names() {
                    if let Some(val) = response.header(&name) {
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
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status,
                    error: None,
                    tags,
                });

                Ok(SyncHttpResponse { status, body, headers, timings })
            },
            Err(e) => {
                let duration = start.elapsed();
                let timings = RequestTimings { duration, ..Default::default() };

                let metric_name: Cow<str> = match &name_tag {
                    Some(n) => Cow::Borrowed(n.as_str()),
                    None => Cow::Borrowed(&url_str),
                };

                let error_msg = e.to_string();
                let _ = tx.send(Metric::Request {
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status: 0,
                    error: Some(error_msg.clone()),
                    tags,
                });

                Ok(SyncHttpResponse {
                    status: 0,
                    body: error_msg.into_bytes(),
                    headers: HashMap::new(),
                    timings
                })
            }
        }
    }))?;

    // POST
    let tx_post = tx.clone();
    let sink_post = global_response_sink;
    http.set("post", Function::new(ctx.clone(), move |url_str: String, body: String, rest: rquickjs::function::Rest<rquickjs::Value>| -> Result<SyncHttpResponse> {
        let name_tag: Option<String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get::<_, String>("name").ok());

        let tags: HashMap<String, String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get("tags").ok())
            .unwrap_or_default();

        let content_type: String = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get::<_, HashMap<String, String>>("headers").ok())
            .and_then(|h| h.get("Content-Type").cloned())
            .unwrap_or_else(|| "application/json".to_string());

        let tx = tx_post.clone();
        let start = Instant::now();

        let result = AGENT.with(|agent| {
            agent.post(&url_str)
                .set("Content-Type", &content_type)
                .send_string(&body)
        });

        let waiting = start.elapsed();

        match result {
            Ok(response) => {
                let status = response.status();
                let mut headers = HashMap::new();
                for name in response.headers_names() {
                    if let Some(val) = response.header(&name) {
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
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status,
                    error: None,
                    tags,
                });

                Ok(SyncHttpResponse { status, body: resp_body, headers, timings })
            },
            Err(e) => {
                let duration = start.elapsed();
                let timings = RequestTimings { duration, ..Default::default() };

                let metric_name: Cow<str> = match &name_tag {
                    Some(n) => Cow::Borrowed(n.as_str()),
                    None => Cow::Borrowed(&url_str),
                };

                let error_msg = e.to_string();
                let _ = tx.send(Metric::Request {
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status: 0,
                    error: Some(error_msg.clone()),
                    tags,
                });

                Ok(SyncHttpResponse {
                    status: 0,
                    body: error_msg.into_bytes(),
                    headers: HashMap::new(),
                    timings
                })
            }
        }
    }))?;

    // PUT
    let tx_put = tx.clone();
    let sink_put = global_response_sink;
    http.set("put", Function::new(ctx.clone(), move |url_str: String, body: String, rest: rquickjs::function::Rest<rquickjs::Value>| -> Result<SyncHttpResponse> {
        let name_tag: Option<String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get::<_, String>("name").ok());

        let tags: HashMap<String, String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get("tags").ok())
            .unwrap_or_default();

        let tx = tx_put.clone();
        let start = Instant::now();

        let result = AGENT.with(|agent| {
            agent.put(&url_str)
                .set("Content-Type", "application/json")
                .send_string(&body)
        });

        let duration = start.elapsed();
        let timings = RequestTimings { duration, ..Default::default() };

        match result {
            Ok(response) => {
                let status = response.status();
                let mut headers = HashMap::new();
                for name in response.headers_names() {
                    if let Some(val) = response.header(&name) {
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
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status,
                    error: None,
                    tags,
                });

                Ok(SyncHttpResponse { status, body: resp_body, headers, timings })
            },
            Err(e) => {
                let metric_name: Cow<str> = match &name_tag {
                    Some(n) => Cow::Borrowed(n.as_str()),
                    None => Cow::Borrowed(&url_str),
                };
                let error_msg = e.to_string();
                let _ = tx.send(Metric::Request {
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status: 0,
                    error: Some(error_msg.clone()),
                    tags,
                });
                Ok(SyncHttpResponse { status: 0, body: error_msg.into_bytes(), headers: HashMap::new(), timings })
            }
        }
    }))?;

    // DELETE
    let tx_del = tx.clone();
    let sink_del = global_response_sink;
    http.set("del", Function::new(ctx.clone(), move |url_str: String, rest: rquickjs::function::Rest<rquickjs::Value>| -> Result<SyncHttpResponse> {
        let name_tag: Option<String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get::<_, String>("name").ok());

        let tags: HashMap<String, String> = rest.first()
            .and_then(|arg| arg.as_object())
            .and_then(|obj| obj.get("tags").ok())
            .unwrap_or_default();

        let tx = tx_del.clone();
        let start = Instant::now();

        let result = AGENT.with(|agent| {
            agent.delete(&url_str).call()
        });

        let duration = start.elapsed();
        let timings = RequestTimings { duration, ..Default::default() };

        match result {
            Ok(response) => {
                let status = response.status();
                let mut headers = HashMap::new();
                for name in response.headers_names() {
                    if let Some(val) = response.header(&name) {
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
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status,
                    error: None,
                    tags,
                });

                Ok(SyncHttpResponse { status, body: resp_body, headers, timings })
            },
            Err(e) => {
                let metric_name: Cow<str> = match &name_tag {
                    Some(n) => Cow::Borrowed(n.as_str()),
                    None => Cow::Borrowed(&url_str),
                };
                let error_msg = e.to_string();
                let _ = tx.send(Metric::Request {
                    name: format!("{}{}", crate::bridge::group::get_current_group_prefix(), metric_name),
                    timings,
                    status: 0,
                    error: Some(error_msg.clone()),
                    tags,
                });
                Ok(SyncHttpResponse { status: 0, body: error_msg.into_bytes(), headers: HashMap::new(), timings })
            }
        }
    }))?;

    ctx.globals().set("http", http)?;
    Ok(())
}
