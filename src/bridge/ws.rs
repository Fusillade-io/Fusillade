use crate::stats::{Metric, RequestTimings};
use crossbeam_channel::Sender;
use rquickjs::{class::Trace, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::TcpStream;
use std::time::Instant;
use tungstenite::{connect, stream::MaybeTlsStream, Message};

type InnerWebSocket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

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

#[derive(Trace)]
#[rquickjs::class]
pub struct JsWebSocket {
    #[qjs(skip_trace)]
    inner: RefCell<Option<InnerWebSocket>>,
    #[qjs(skip_trace)]
    tx: Sender<Metric>,
}

unsafe impl<'js> JsLifetime<'js> for JsWebSocket {
    type Changed<'to> = JsWebSocket;
}

#[rquickjs::methods]
impl JsWebSocket {
    pub fn send(&self, msg: String) -> Result<()> {
        let start = Instant::now();
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            match ws.send(Message::Text(msg.into())) {
                Ok(()) => {
                    let _ = self.tx.send(make_metric("ws::send", start, None));
                    Ok(())
                }
                Err(e) => {
                    let err_msg = format!("WS Send Error: {}", e);
                    let _ = self
                        .tx
                        .send(make_metric("ws::send", start, Some(err_msg.clone())));
                    eprintln!("{}", err_msg);
                    Err(rquickjs::Error::Exception)
                }
            }
        } else {
            let _ = self.tx.send(make_metric(
                "ws::send",
                start,
                Some("WebSocket not connected".to_string()),
            ));
            Err(rquickjs::Error::Exception)
        }
    }

    #[qjs(rename = "sendBinary")]
    pub fn send_binary(&self, data: String) -> Result<()> {
        use base64::Engine;
        let start = Instant::now();
        let decoded = match base64::engine::general_purpose::STANDARD.decode(&data) {
            Ok(bytes) => bytes,
            Err(e) => {
                let err_msg = format!("Base64 decode error: {}", e);
                let _ = self
                    .tx
                    .send(make_metric("ws::sendBinary", start, Some(err_msg.clone())));
                eprintln!("{}", err_msg);
                return Err(rquickjs::Error::Exception);
            }
        };
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            match ws.send(Message::Binary(decoded.into())) {
                Ok(()) => {
                    let _ = self.tx.send(make_metric("ws::sendBinary", start, None));
                    Ok(())
                }
                Err(e) => {
                    let err_msg = format!("WS SendBinary Error: {}", e);
                    let _ = self
                        .tx
                        .send(make_metric("ws::sendBinary", start, Some(err_msg.clone())));
                    eprintln!("{}", err_msg);
                    Err(rquickjs::Error::Exception)
                }
            }
        } else {
            let _ = self.tx.send(make_metric(
                "ws::sendBinary",
                start,
                Some("WebSocket not connected".to_string()),
            ));
            Err(rquickjs::Error::Exception)
        }
    }

    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let start = Instant::now();
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            match ws.read() {
                Ok(msg) => {
                    let _ = self.tx.send(make_metric("ws::recv", start, None));
                    match msg {
                        Message::Text(s) => {
                            // Return text messages as string for backwards compatibility
                            s.to_string().into_js(&ctx)
                        }
                        Message::Binary(b) => {
                            // Return binary data as object with type indicator and base64 data
                            use base64::Engine;
                            let obj = Object::new(ctx.clone())?;
                            obj.set("type", "binary")?;
                            obj.set("data", base64::engine::general_purpose::STANDARD.encode(&b))?;
                            Ok(obj.into_value())
                        }
                        Message::Close(_) => {
                            *borrow = None;
                            Ok(Value::new_null(ctx))
                        }
                        _ => Ok(Value::new_undefined(ctx)),
                    }
                }
                Err(e) => {
                    let err_msg = format!("WS Recv Error: {}", e);
                    let _ = self
                        .tx
                        .send(make_metric("ws::recv", start, Some(err_msg.clone())));
                    eprintln!("{}", err_msg);
                    Err(rquickjs::Error::Exception)
                }
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    pub fn close(&self) -> Result<()> {
        let mut borrow = self.inner.borrow_mut();
        if let Some(mut ws) = borrow.take() {
            let _ = ws.close(None);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct WsMetricSender(Sender<Metric>);
unsafe impl<'js> JsLifetime<'js> for WsMetricSender {
    type Changed<'to> = WsMetricSender;
}

fn ws_connect<'js>(ctx: Ctx<'js>, url: String) -> Result<Value<'js>> {
    let tx = ctx.userdata::<WsMetricSender>().map(|w| w.0.clone());
    let start = Instant::now();
    match connect(&url) {
        Ok((socket, _response)) => {
            if let Some(ref tx) = tx {
                let _ = tx.send(make_metric("ws::connect", start, None));
            }
            let ws = JsWebSocket {
                inner: RefCell::new(Some(socket)),
                tx: tx.unwrap_or_else(|| {
                    // Fallback: create a dummy sender (should not happen in practice)
                    let (s, _) = crossbeam_channel::unbounded();
                    s
                }),
            };
            let instance = rquickjs::Class::<JsWebSocket>::instance(ctx.clone(), ws)?;
            Ok(instance.into_value())
        }
        Err(e) => {
            let err_msg = format!("WebSocket connection failed: {}", e);
            if let Some(ref tx) = tx {
                let _ = tx.send(make_metric("ws::connect", start, Some(err_msg.clone())));
            }
            let err_val = err_msg.into_js(&ctx)?;
            Err(ctx.throw(err_val))
        }
    }
}

impl Drop for JsWebSocket {
    fn drop(&mut self) {
        // Clean up WebSocket connection when JS object is garbage collected
        if let Ok(mut borrow) = self.inner.try_borrow_mut() {
            if let Some(mut ws) = borrow.take() {
                let _ = ws.close(None);
            }
        }
    }
}

pub fn register_sync(ctx: &Ctx, tx: Sender<Metric>) -> Result<()> {
    ctx.store_userdata(WsMetricSender(tx))?;
    rquickjs::Class::<JsWebSocket>::define(&ctx.globals())?;

    let ws_mod = Object::new(ctx.clone())?;
    ws_mod.set("connect", Function::new(ctx.clone(), ws_connect))?;

    ctx.globals().set("ws", ws_mod)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_make_metric_success() {
        let start = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        let metric = make_metric("ws::connect", start, None);

        match metric {
            Metric::Request {
                name,
                timings,
                status,
                error,
                tags,
            } => {
                assert_eq!(name, "ws::connect");
                assert_eq!(status, 200);
                assert!(error.is_none());
                assert!(timings.duration >= Duration::from_millis(5));
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_make_metric_error() {
        let start = Instant::now();
        let error_msg = "Connection refused".to_string();
        let metric = make_metric("ws::connect", start, Some(error_msg.clone()));

        match metric {
            Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "ws::connect");
                assert_eq!(status, 0);
                assert_eq!(error, Some(error_msg));
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_base64_decode_for_send_binary() {
        use base64::Engine;
        let input = "SGVsbG8sIFdvcmxkIQ=="; // "Hello, World!" in base64
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(input)
            .unwrap();
        assert_eq!(decoded, b"Hello, World!");
    }

    #[test]
    fn test_base64_decode_invalid() {
        use base64::Engine;
        let result = base64::engine::general_purpose::STANDARD.decode("not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_ws_metrics_sent_to_channel() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let start = Instant::now();
        let metric = make_metric("ws::send", start, None);
        tx.send(metric).unwrap();

        let received = rx.recv().unwrap();
        match received {
            Metric::Request { name, .. } => {
                assert_eq!(name, "ws::send");
            }
            _ => panic!("Expected Request metric"),
        }
    }
}
