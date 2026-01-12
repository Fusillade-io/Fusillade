use rquickjs::{Ctx, Function, Object, Result, JsLifetime, Value, IntoJs, class::Trace};
use tungstenite::{connect, Message, stream::MaybeTlsStream};
use std::net::TcpStream;
use std::cell::RefCell;

type InnerWebSocket = tungstenite::WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Trace)]
#[rquickjs::class]
pub struct JsWebSocket {
    #[qjs(skip_trace)]
    inner: RefCell<Option<InnerWebSocket>>,
}

unsafe impl<'js> JsLifetime<'js> for JsWebSocket {
    type Changed<'to> = JsWebSocket;
}

#[rquickjs::methods]
impl JsWebSocket {
    pub fn send(&self, msg: String) -> Result<()> {
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            ws.send(Message::Text(msg.into())).map_err(|e| {
                eprintln!("WS Send Error: {}", e);
                rquickjs::Error::Exception
            })?;
            Ok(())
        } else {
            Err(rquickjs::Error::Exception)
        }
    }

    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut borrow = self.inner.borrow_mut();
        if let Some(ws) = borrow.as_mut() {
            match ws.read() {
                Ok(msg) => {
                    match msg {
                        Message::Text(s) => s.to_string().into_js(&ctx),
                        Message::Binary(b) => String::from_utf8_lossy(&b).to_string().into_js(&ctx),
                        Message::Close(_) => {
                            *borrow = None;
                            Ok(Value::new_null(ctx))
                        },
                        _ => Ok(Value::new_undefined(ctx)),
                    }
                },
                Err(e) => {
                    eprintln!("WS Recv Error: {}", e);
                    Err(rquickjs::Error::Exception)
                },
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

fn ws_connect<'js>(ctx: Ctx<'js>, url: String) -> Result<Value<'js>> {
    match connect(&url) {
        Ok((socket, _response)) => {
            let ws = JsWebSocket {
                inner: RefCell::new(Some(socket)),
            };
            let instance = rquickjs::Class::<JsWebSocket>::instance(ctx.clone(), ws)?;
            Ok(instance.into_value())
        },
        Err(e) => {
            let msg = format!("WebSocket connection failed: {}", e);
            let err_val = msg.into_js(&ctx)?;
            Err(ctx.throw(err_val))
        }
    }
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    rquickjs::Class::<JsWebSocket>::define(&ctx.globals())?;

    let ws_mod = Object::new(ctx.clone())?;
    ws_mod.set("connect", Function::new(ctx.clone(), ws_connect))?;

    ctx.globals().set("ws", ws_mod)?;
    Ok(())
}