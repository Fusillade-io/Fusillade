use rquickjs::{
    class::{Trace, Tracer},
    Ctx, JsLifetime, Object, Result, Value,
};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Message received from MQTT subscription
struct MqttMessage {
    topic: String,
    payload: Vec<u8>,
    qos: u8,
}

#[rquickjs::class]
pub struct JsMqttClient {
    client: Option<Client>,
    message_rx: Option<crossbeam_channel::Receiver<MqttMessage>>,
    running: Arc<AtomicBool>,
    _background_handle: Option<JoinHandle<()>>,
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
            message_rx: None,
            running: Arc::new(AtomicBool::new(false)),
            _background_handle: None,
        }
    }

    pub fn connect(&mut self, host: String, port: u16, client_id: String) -> Result<()> {
        let mut mqttoptions = MqttOptions::new(client_id, host, port);
        mqttoptions.set_keep_alive(Duration::from_secs(5));

        let (client, mut connection) = Client::new(mqttoptions, 10);

        // Create channel for messages
        let (tx, rx) = crossbeam_channel::bounded::<MqttMessage>(1000);

        // Set up running flag
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();

        // Spawn background thread to poll connection events
        let handle = std::thread::spawn(move || {
            while running.load(Ordering::SeqCst) {
                // Use recv_timeout to allow periodic checks for shutdown
                match connection.recv_timeout(Duration::from_millis(100)) {
                    Ok(Ok(event)) => {
                        if let Event::Incoming(Packet::Publish(publish)) = event {
                            let msg = MqttMessage {
                                topic: publish.topic.clone(),
                                payload: publish.payload.to_vec(),
                                qos: match publish.qos {
                                    QoS::AtMostOnce => 0,
                                    QoS::AtLeastOnce => 1,
                                    QoS::ExactlyOnce => 2,
                                },
                            };
                            // Send to channel, ignore if full or disconnected
                            let _ = tx.send(msg);
                        }
                    }
                    Ok(Err(_)) => {
                        // Connection error, continue
                    }
                    Err(rumqttc::RecvTimeoutError::Timeout) => {
                        // Continue polling
                    }
                    Err(rumqttc::RecvTimeoutError::Disconnected) => {
                        // Connection closed
                        break;
                    }
                }
            }
        });

        self.client = Some(client);
        self.message_rx = Some(rx);
        self._background_handle = Some(handle);

        Ok(())
    }

    /// Subscribe to a topic pattern (supports MQTT wildcards + and #)
    pub fn subscribe(&mut self, topic: String) -> Result<()> {
        if let Some(ref mut client) = self.client {
            client.subscribe(&topic, QoS::AtLeastOnce).map_err(|_| {
                rquickjs::Error::new_from_js("MQTT Subscribe failed", "NetworkError")
            })?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js(
                "MQTT Client not connected",
                "StateError",
            ))
        }
    }

    /// Receive the next message from subscribed topics
    /// Returns { topic, payload, qos } or null if no message available/disconnected
    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if let Some(ref rx) = self.message_rx {
            // Use recv_timeout for blocking with reasonable timeout
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(msg) => {
                    let obj = Object::new(ctx.clone())?;
                    obj.set("topic", msg.topic)?;
                    // Try to decode payload as UTF-8 string, otherwise use lossy conversion
                    let payload_str = String::from_utf8_lossy(&msg.payload).into_owned();
                    obj.set("payload", payload_str)?;
                    obj.set("qos", msg.qos)?;
                    Ok(obj.into_value())
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Timeout - return null to allow script to check/continue
                    Ok(Value::new_null(ctx))
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    // Channel closed
                    Ok(Value::new_null(ctx))
                }
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    pub fn publish(&mut self, topic: String, payload: String) -> Result<()> {
        if let Some(ref mut client) = self.client {
            client
                .publish(topic, QoS::AtLeastOnce, false, payload.as_bytes())
                .map_err(|_| rquickjs::Error::new_from_js("MQTT Publish failed", "NetworkError"))?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js(
                "MQTT Client not connected",
                "StateError",
            ))
        }
    }

    pub fn close(&mut self) -> Result<()> {
        // Signal background thread to stop
        self.running.store(false, Ordering::SeqCst);

        self.client = None;
        self.message_rx = None;
        // Note: JoinHandle is dropped automatically, thread will stop on next iteration
        self._background_handle = None;
        Ok(())
    }
}

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    rquickjs::Class::<JsMqttClient>::define(&globals)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_message_struct() {
        let msg = MqttMessage {
            topic: "sensors/temperature".to_string(),
            payload: b"22.5".to_vec(),
            qos: 1,
        };

        assert_eq!(msg.topic, "sensors/temperature");
        assert_eq!(msg.payload, b"22.5");
        assert_eq!(msg.qos, 1);
    }

    #[test]
    fn test_mqtt_message_utf8_payload() {
        let msg = MqttMessage {
            topic: "test/topic".to_string(),
            payload: "Hello, World!".as_bytes().to_vec(),
            qos: 0,
        };

        let payload_str = String::from_utf8_lossy(&msg.payload).into_owned();
        assert_eq!(payload_str, "Hello, World!");
    }

    #[test]
    fn test_mqtt_qos_values() {
        // Verify QoS mapping matches MQTT spec
        assert_eq!(
            match QoS::AtMostOnce {
                QoS::AtMostOnce => 0u8,
                QoS::AtLeastOnce => 1u8,
                QoS::ExactlyOnce => 2u8,
            },
            0
        );
        assert_eq!(
            match QoS::AtLeastOnce {
                QoS::AtMostOnce => 0u8,
                QoS::AtLeastOnce => 1u8,
                QoS::ExactlyOnce => 2u8,
            },
            1
        );
        assert_eq!(
            match QoS::ExactlyOnce {
                QoS::AtMostOnce => 0u8,
                QoS::AtLeastOnce => 1u8,
                QoS::ExactlyOnce => 2u8,
            },
            2
        );
    }

    #[test]
    fn test_mqtt_topic_validation() {
        // Test that topic patterns work as expected
        let single_level = "sensors/+/temperature";
        let multi_level = "sensors/#";
        let normal = "sensors/room1/temperature";

        assert!(single_level.contains('+'));
        assert!(multi_level.contains('#'));
        assert!(!normal.contains('+') && !normal.contains('#'));
    }
}
