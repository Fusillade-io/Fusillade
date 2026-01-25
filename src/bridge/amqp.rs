use lapin::{options::*, BasicProperties, Connection, ConnectionProperties};
use rquickjs::{
    class::{Trace, Tracer},
    Ctx, JsLifetime, Result,
};
use tokio::runtime::Runtime;

#[rquickjs::class]
pub struct JsAmqpClient {
    connection: Option<Connection>,
    channel: Option<lapin::Channel>,
    runtime: Arc<Runtime>,
}

unsafe impl<'js> JsLifetime<'js> for JsAmqpClient {
    type Changed<'to> = JsAmqpClient;
}

impl<'js> Trace<'js> for JsAmqpClient {
    fn trace(&self, _tracer: Tracer<'_, 'js>) {}
}

#[rquickjs::methods]
impl JsAmqpClient {
    #[qjs(constructor)]
    pub fn new() -> Self {
        // Create a dedicated runtime for AMQP tasks since bridge is sync
        let rt = Runtime::new().expect("Failed to create AMQP runtime");
        Self {
            connection: None,
            channel: None,
            runtime: Arc::new(rt),
        }
    }

    pub fn connect(&mut self, url: String) -> Result<()> {
        let rt = self.runtime.clone();
        let connection = rt
            .block_on(async { Connection::connect(&url, ConnectionProperties::default()).await })
            .map_err(|_e| rquickjs::Error::new_from_js("AMQP Connect failed", "NetworkError"))?;

        let channel = rt
            .block_on(async { connection.create_channel().await })
            .map_err(|_e| {
                rquickjs::Error::new_from_js("AMQP Channel creation failed", "NetworkError")
            })?;

        self.connection = Some(connection);
        self.channel = Some(channel);
        Ok(())
    }

    pub fn publish(
        &mut self,
        exchange: String,
        routing_key: String,
        payload: String,
    ) -> Result<()> {
        if let Some(ref channel) = self.channel {
            let rt = self.runtime.clone();
            rt.block_on(async {
                channel
                    .basic_publish(
                        &exchange,
                        &routing_key,
                        BasicPublishOptions::default(),
                        payload.as_bytes(),
                        BasicProperties::default(),
                    )
                    .await
            })
            .map_err(|_| rquickjs::Error::new_from_js("AMQP Publish failed", "NetworkError"))?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js(
                "AMQP Client not connected",
                "StateError",
            ))
        }
    }

    pub fn close(&mut self) -> Result<()> {
        self.connection = None;
        self.channel = None;
        Ok(())
    }
}

use std::sync::Arc;

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    rquickjs::Class::<JsAmqpClient>::define(&globals)?;
    Ok(())
}
