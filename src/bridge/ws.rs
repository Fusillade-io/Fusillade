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

    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let start = Instant::now();
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            match ws.read() {
                Ok(msg) => {
                    let _ = self.tx.send(make_metric("ws::recv", start, None));
                    match msg {
                        Message::Text(s) => s.to_string().into_js(&ctx),
                        Message::Binary(b) => String::from_utf8_lossy(&b).to_string().into_js(&ctx),
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

pub fn register_sync(ctx: &Ctx, tx: Sender<Metric>) -> Result<()> {
    ctx.store_userdata(WsMetricSender(tx))?;
    rquickjs::Class::<JsWebSocket>::define(&ctx.globals())?;

    let ws_mod = Object::new(ctx.clone())?;
    ws_mod.set("connect", Function::new(ctx.clone(), ws_connect))?;

    ctx.globals().set("ws", ws_mod)?;
    Ok(())
}
