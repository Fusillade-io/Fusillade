//! Server-Sent Events (SSE) Bridge
//!
//! Exposes SSE client functionality to JavaScript for load testing SSE endpoints.

use crate::stats::{Metric, RequestTimings};
use crossbeam_channel::Sender;
use rquickjs::{class::Trace, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::time::Instant;

fn make_metric(name: &str, start: Instant, error: Option<String>) -> Metric {
    let duration = start.elapsed();
    let timings = RequestTimings {
        duration,
        waiting: duration,
        ..Default::default()
    };
    let status = if error.is_none() { 200 } else { 0 };
    Metric::Request {
        name: name.to_string(),
        timings,
        status,
        error,
        tags: HashMap::new(),
    }
}

/// SSE Client wrapping a reqwest blocking response
#[derive(Trace)]
#[rquickjs::class]
pub struct JsSseClient {
    #[qjs(skip_trace)]
    inner: RefCell<Option<SseStream>>,
    #[qjs(skip_trace)]
    url: String,
    #[qjs(skip_trace)]
    tx: Sender<Metric>,
}

struct SseStream {
    reader: BufReader<reqwest::blocking::Response>,
    last_event_id: Option<String>,
}

unsafe impl<'js> JsLifetime<'js> for JsSseClient {
    type Changed<'to> = JsSseClient;
}

#[rquickjs::methods]
impl JsSseClient {
    /// Receive the next SSE event
    /// Returns an object { event, data, id } or null if disconnected
    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let start = Instant::now();
        let mut borrow = self.inner.borrow_mut();
        if let Some(stream) = borrow.as_mut() {
            let event = read_sse_event(&mut stream.reader);
            match event {
                Ok(Some(sse_event)) => {
                    let _ = self.tx.send(make_metric("sse::recv", start, None));
                    if let Some(id) = &sse_event.id {
                        stream.last_event_id = Some(id.clone());
                    }

                    let obj = Object::new(ctx.clone())?;
                    obj.set(
                        "event",
                        sse_event.event.unwrap_or_else(|| "message".to_string()),
                    )?;
                    obj.set("data", sse_event.data)?;
                    if let Some(id) = sse_event.id {
                        obj.set("id", id)?;
                    }
                    Ok(obj.into_value())
                }
                Ok(None) => {
                    // Stream ended
                    *borrow = None;
                    Ok(Value::new_null(ctx))
                }
                Err(e) => {
                    let err_msg = format!("SSE Read Error: {}", e);
                    let _ = self
                        .tx
                        .send(make_metric("sse::recv", start, Some(err_msg.clone())));
                    eprintln!("{}", err_msg);
                    *borrow = None;
                    Ok(Value::new_null(ctx))
                }
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    /// Close the SSE connection
    pub fn close(&self) -> Result<()> {
        let mut borrow = self.inner.borrow_mut();
        *borrow = None;
        Ok(())
    }

    /// Get the URL of this connection
    #[qjs(get)]
    pub fn url(&self) -> String {
        self.url.clone()
    }
}

/// Represents a single SSE event
struct SseEvent {
    event: Option<String>,
    data: String,
    id: Option<String>,
}

/// Read a single SSE event from the stream
fn read_sse_event<R: BufRead>(reader: &mut R) -> std::io::Result<Option<SseEvent>> {
    let mut event_type: Option<String> = None;
    let mut data_lines: Vec<String> = Vec::new();
    let mut id: Option<String> = None;
    let mut has_content = false;

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;

        if bytes_read == 0 {
            // EOF
            if has_content {
                return Ok(Some(SseEvent {
                    event: event_type,
                    data: data_lines.join("\n"),
                    id,
                }));
            }
            return Ok(None);
        }

        let line = line.trim_end_matches(['\r', '\n'].as_ref());

        if line.is_empty() {
            // Empty line = end of event
            if has_content {
                return Ok(Some(SseEvent {
                    event: event_type,
                    data: data_lines.join("\n"),
                    id,
                }));
            }
            continue;
        }

        // Skip comment lines
        if line.starts_with(':') {
            continue;
        }

        has_content = true;

        if let Some(value) = line.strip_prefix("event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("id:") {
            id = Some(value.trim().to_string());
        }
        // Ignore retry: and unknown fields
    }
}

#[derive(Clone)]
struct SseMetricSender(Sender<Metric>);
unsafe impl<'js> JsLifetime<'js> for SseMetricSender {
    type Changed<'to> = SseMetricSender;
}

/// Connect to an SSE endpoint
fn sse_connect<'js>(ctx: Ctx<'js>, url: String) -> Result<Value<'js>> {
    let tx = ctx.userdata::<SseMetricSender>().map(|w| w.0.clone());
    let start = Instant::now();
    let client = reqwest::blocking::Client::new();

    let response = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .send();

    match response {
        Ok(resp) => {
            if !resp.status().is_success() {
                let msg = format!("SSE connection failed: HTTP {}", resp.status());
                if let Some(ref tx) = tx {
                    let _ = tx.send(make_metric("sse::connect", start, Some(msg.clone())));
                }
                let err_val = msg.into_js(&ctx)?;
                return Err(ctx.throw(err_val));
            }

            if let Some(ref tx) = tx {
                let _ = tx.send(make_metric("sse::connect", start, None));
            }

            let stream = SseStream {
                reader: BufReader::new(resp),
                last_event_id: None,
            };

            let metric_tx = tx.unwrap_or_else(|| {
                let (s, _) = crossbeam_channel::unbounded();
                s
            });

            let client = JsSseClient {
                inner: RefCell::new(Some(stream)),
                url: url.clone(),
                tx: metric_tx,
            };

            let instance = rquickjs::Class::<JsSseClient>::instance(ctx.clone(), client)?;
            Ok(instance.into_value())
        }
        Err(e) => {
            let msg = format!("SSE connection failed: {}", e);
            if let Some(ref tx) = tx {
                let _ = tx.send(make_metric("sse::connect", start, Some(msg.clone())));
            }
            let err_val = msg.into_js(&ctx)?;
            Err(ctx.throw(err_val))
        }
    }
}

impl Drop for JsSseClient {
    fn drop(&mut self) {
        // Clean up SSE connection when JS object is garbage collected
        if let Ok(mut borrow) = self.inner.try_borrow_mut() {
            *borrow = None;
        }
    }
}

pub fn register_sync(ctx: &Ctx, tx: Sender<Metric>) -> Result<()> {
    ctx.store_userdata(SseMetricSender(tx))?;
    rquickjs::Class::<JsSseClient>::define(&ctx.globals())?;

    let sse_mod = Object::new(ctx.clone())?;
    sse_mod.set("connect", Function::new(ctx.clone(), sse_connect))?;

    ctx.globals().set("sse", sse_mod)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_event_parsing_basic() {
        let data = b"event: message\ndata: hello world\nid: 1\n\n";
        let response = MockResponse(data.to_vec());
        let mut reader = BufReader::new(response);

        let event = read_sse_event(&mut reader).unwrap();
        assert!(event.is_some());

        let event = event.unwrap();
        assert_eq!(event.event, Some("message".to_string()));
        assert_eq!(event.data, "hello world");
        assert_eq!(event.id, Some("1".to_string()));
    }

    #[test]
    fn test_sse_event_parsing_data_only() {
        let data = b"data: just data\n\n";
        let response = MockResponse(data.to_vec());
        let mut reader = BufReader::new(response);

        let event = read_sse_event(&mut reader).unwrap();
        assert!(event.is_some());

        let event = event.unwrap();
        assert!(event.event.is_none());
        assert_eq!(event.data, "just data");
    }

    #[test]
    fn test_sse_event_parsing_multiline_data() {
        let data = b"data: line1\ndata: line2\ndata: line3\n\n";
        let response = MockResponse(data.to_vec());
        let mut reader = BufReader::new(response);

        let event = read_sse_event(&mut reader).unwrap();
        assert!(event.is_some());

        let event = event.unwrap();
        assert_eq!(event.data, "line1\nline2\nline3");
    }

    #[test]
    fn test_sse_event_parsing_comment_ignored() {
        let data = b": this is a comment\ndata: actual data\n\n";
        let response = MockResponse(data.to_vec());
        let mut reader = BufReader::new(response);

        let event = read_sse_event(&mut reader).unwrap();
        assert!(event.is_some());

        let event = event.unwrap();
        assert_eq!(event.data, "actual data");
    }

    #[test]
    fn test_sse_event_parsing_eof() {
        let data = b"";
        let response = MockResponse(data.to_vec());
        let mut reader = BufReader::new(response);

        let event = read_sse_event(&mut reader).unwrap();
        assert!(event.is_none());
    }

    // Mock response for testing
    struct MockResponse(Vec<u8>);

    impl std::io::Read for MockResponse {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let len = std::cmp::min(buf.len(), self.0.len());
            buf[..len].copy_from_slice(&self.0[..len]);
            self.0.drain(..len);
            Ok(len)
        }
    }

    #[test]
    fn test_sse_make_metric_success() {
        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let metric = make_metric("sse::connect", start, None);

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "sse::connect");
                assert_eq!(status, 200);
                assert!(error.is_none());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_sse_make_metric_error() {
        let start = std::time::Instant::now();
        let metric = make_metric("sse::recv", start, Some("Connection closed".to_string()));

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "sse::recv");
                assert_eq!(status, 0);
                assert!(error.is_some());
            }
            _ => panic!("Expected Request metric"),
        }
    }
}
