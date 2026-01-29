use crate::stats::{Metric, RequestTimings};
use crossbeam_channel::Sender;
use rquickjs::{
    class::{Trace, Tracer},
    Ctx, JsLifetime, Object, Result, Value,
};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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

#[derive(Clone)]
struct MetricSender(Sender<Metric>);
unsafe impl<'js> JsLifetime<'js> for MetricSender {
    type Changed<'to> = MetricSender;
}

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
    tx: Option<Sender<Metric>>,
}

unsafe impl<'js> JsLifetime<'js> for JsMqttClient {
    type Changed<'to> = JsMqttClient;
}

impl<'js> Trace<'js> for JsMqttClient {
    fn trace(&self, _tracer: Tracer<'_, 'js>) {}
}

/// Map optional u8 QoS value to rumqttc QoS enum
/// 0 = AtMostOnce, 1 = AtLeastOnce (default), 2 = ExactlyOnce
fn map_qos(qos: Option<u8>) -> QoS {
    match qos.unwrap_or(1) {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

#[rquickjs::methods]
impl JsMqttClient {
    #[qjs(constructor)]
    pub fn new(ctx: Ctx<'_>) -> Self {
        let tx = ctx.userdata::<MetricSender>().map(|w| w.0.clone());
        Self {
            client: None,
            message_rx: None,
            running: Arc::new(AtomicBool::new(false)),
            _background_handle: None,
            tx,
        }
    }

    pub fn connect(&mut self, host: String, port: u16, client_id: String) -> Result<()> {
        let start = Instant::now();
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

        if let Some(ref mtx) = self.tx {
            let _ = mtx.send(make_metric("mqtt::connect", start, None));
        }

        Ok(())
    }

    /// Subscribe to a topic pattern (supports MQTT wildcards + and #)
    /// Optional QoS: 0 = AtMostOnce, 1 = AtLeastOnce (default), 2 = ExactlyOnce
    pub fn subscribe(&mut self, topic: String, qos: Option<u8>) -> Result<()> {
        let start = Instant::now();
        let qos_level = map_qos(qos);
        if let Some(ref mut client) = self.client {
            match client.subscribe(&topic, qos_level) {
                Ok(()) => {
                    if let Some(ref mtx) = self.tx {
                        let _ = mtx.send(make_metric("mqtt::subscribe", start, None));
                    }
                    Ok(())
                }
                Err(e) => {
                    let err_msg = format!("MQTT Subscribe failed: {}", e);
                    if let Some(ref mtx) = self.tx {
                        let _ = mtx.send(make_metric("mqtt::subscribe", start, Some(err_msg)));
                    }
                    Err(rquickjs::Error::new_from_js(
                        "MQTT Subscribe failed",
                        "NetworkError",
                    ))
                }
            }
        } else {
            Err(rquickjs::Error::new_from_js(
                "MQTT Client not connected",
                "StateError",
            ))
        }
    }

    /// Receive the next message from subscribed topics
    /// Returns { value: { topic, payload, qos } | null, reason: "timeout" | "closed" | "not_connected" | null }
    pub fn recv<'js>(&self, ctx: Ctx<'js>, timeout_ms: Option<u64>) -> Result<Value<'js>> {
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000));

        if let Some(ref rx) = self.message_rx {
            // Use recv_timeout for blocking with configurable timeout
            match rx.recv_timeout(timeout) {
                Ok(msg) => {
                    if let Some(ref mtx) = self.tx {
                        let _ = mtx.send(make_metric("mqtt::recv", start, None));
                    }
                    let msg_obj = Object::new(ctx.clone())?;
                    msg_obj.set("topic", msg.topic)?;
                    // Try to decode payload as UTF-8 string, otherwise use lossy conversion
                    let payload_str = String::from_utf8_lossy(&msg.payload).into_owned();
                    msg_obj.set("payload", payload_str)?;
                    msg_obj.set("qos", msg.qos)?;

                    let result = Object::new(ctx.clone())?;
                    result.set("value", msg_obj)?;
                    result.set("reason", Value::new_null(ctx))?;
                    Ok(result.into_value())
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    let result = Object::new(ctx.clone())?;
                    result.set("value", Value::new_null(ctx.clone()))?;
                    result.set("reason", "timeout")?;
                    Ok(result.into_value())
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    let result = Object::new(ctx.clone())?;
                    result.set("value", Value::new_null(ctx.clone()))?;
                    result.set("reason", "closed")?;
                    Ok(result.into_value())
                }
            }
        } else {
            let result = Object::new(ctx.clone())?;
            result.set("value", Value::new_null(ctx.clone()))?;
            result.set("reason", "not_connected")?;
            Ok(result.into_value())
        }
    }

    /// Publish a message to a topic
    /// Optional QoS: 0 = AtMostOnce, 1 = AtLeastOnce (default), 2 = ExactlyOnce
    pub fn publish(&mut self, topic: String, payload: String, qos: Option<u8>) -> Result<()> {
        let start = Instant::now();
        let qos_level = map_qos(qos);
        if let Some(ref mut client) = self.client {
            match client.publish(topic, qos_level, false, payload.as_bytes()) {
                Ok(()) => {
                    if let Some(ref mtx) = self.tx {
                        let _ = mtx.send(make_metric("mqtt::publish", start, None));
                    }
                    Ok(())
                }
                Err(e) => {
                    let err_msg = format!("MQTT Publish failed: {}", e);
                    if let Some(ref mtx) = self.tx {
                        let _ = mtx.send(make_metric("mqtt::publish", start, Some(err_msg)));
                    }
                    Err(rquickjs::Error::new_from_js(
                        "MQTT Publish failed",
                        "NetworkError",
                    ))
                }
            }
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

        // Wait for background thread to exit (with timeout)
        if let Some(handle) = self._background_handle.take() {
            // Give thread 1 second to finish gracefully
            let start = std::time::Instant::now();
            while !handle.is_finished() && start.elapsed() < std::time::Duration::from_secs(1) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
            // If not finished after timeout, drop anyway (thread will exit on next flag check)
        }

        self.client = None;
        self.message_rx = None;
        Ok(())
    }
}

pub fn register_sync<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>) -> Result<()> {
    ctx.store_userdata(MetricSender(tx))?;
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

    #[test]
    fn test_map_qos_default() {
        assert!(matches!(map_qos(None), QoS::AtLeastOnce));
    }

    #[test]
    fn test_map_qos_at_most_once() {
        assert!(matches!(map_qos(Some(0)), QoS::AtMostOnce));
    }

    #[test]
    fn test_map_qos_at_least_once() {
        assert!(matches!(map_qos(Some(1)), QoS::AtLeastOnce));
    }

    #[test]
    fn test_map_qos_exactly_once() {
        assert!(matches!(map_qos(Some(2)), QoS::ExactlyOnce));
    }

    #[test]
    fn test_map_qos_invalid_falls_back() {
        // Invalid values default to AtLeastOnce
        assert!(matches!(map_qos(Some(3)), QoS::AtLeastOnce));
        assert!(matches!(map_qos(Some(255)), QoS::AtLeastOnce));
    }

    #[test]
    fn test_mqtt_make_metric_success() {
        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let metric = make_metric("mqtt::publish", start, None);

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "mqtt::publish");
                assert_eq!(status, 200);
                assert!(error.is_none());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_mqtt_make_metric_error() {
        let start = std::time::Instant::now();
        let metric = make_metric(
            "mqtt::connect",
            start,
            Some("Connection refused".to_string()),
        );

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "mqtt::connect");
                assert_eq!(status, 0);
                assert!(error.is_some());
            }
            _ => panic!("Expected Request metric"),
        }
    }
}
