use rquickjs::{Ctx, Result, class::{Trace, Tracer}, JsLifetime};
use rumqttc::{Client, Connection, MqttOptions, QoS};
use std::time::Duration;

#[rquickjs::class]
pub struct JsMqttClient {
    client: Option<Client>,
    connection: Option<Connection>,
}

unsafe impl<'js> JsLifetime<'js> for JsMqttClient {
    type Changed<'to> = JsMqttClient;
}

impl<'js> Trace<'js> for JsMqttClient {
    fn trace(&self, _tracer: Tracer<'_, 'js>) {}
}

#[rquickjs::methods]
impl JsMqttClient {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {
            client: None,
            connection: None,
        }
    }

    pub fn connect(&mut self, host: String, port: u16, client_id: String) -> Result<()> {
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        mqttoptions.set_keep_alive(Duration::from_secs(5));

        let (client, connection) = Client::new(mqttoptions, 10);
        self.client = Some(client);
        self.connection = Some(connection);
        Ok(())
    }

    pub fn publish(&mut self, topic: String, payload: String) -> Result<()> {
        if let Some(ref mut client) = self.client {
            client.publish(topic, QoS::AtLeastOnce, false, payload.as_bytes())
                .map_err(|_| rquickjs::Error::new_from_js("MQTT Publish failed", "NetworkError"))?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js("MQTT Client not connected", "StateError"))
        }
    }

    pub fn close(&mut self) -> Result<()> {
        self.client = None;
        self.connection = None;
        Ok(())
    }
}

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    rquickjs::Class::<JsMqttClient>::define(&globals)?;
    Ok(())
}
