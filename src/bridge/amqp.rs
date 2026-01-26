use futures_lite::StreamExt;
use lapin::{
    options::*, types::FieldTable, BasicProperties, Channel, Connection, ConnectionProperties,
};
use rquickjs::{
    class::{Trace, Tracer},
    Ctx, JsLifetime, Object, Result, Value,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::runtime::Runtime;

/// Message received from AMQP consumer
struct AmqpMessage {
    body: Vec<u8>,
    delivery_tag: u64,
}

#[rquickjs::class]
pub struct JsAmqpClient {
    connection: Option<Connection>,
    channel: Option<Channel>,
    runtime: Arc<Runtime>,
    message_rx: Option<crossbeam_channel::Receiver<AmqpMessage>>,
    running: Arc<AtomicBool>,
    _background_handle: Option<JoinHandle<()>>,
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
            message_rx: None,
            running: Arc::new(AtomicBool::new(false)),
            _background_handle: None,
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

    /// Subscribe to a queue and start consuming messages
    pub fn subscribe(&mut self, queue: String) -> Result<()> {
        let channel = self.channel.as_ref().ok_or_else(|| {
            rquickjs::Error::new_from_js("AMQP Client not connected", "StateError")
        })?;

        let rt = self.runtime.clone();

        // Declare queue (idempotent)
        rt.block_on(async {
            channel
                .queue_declare(
                    &queue,
                    QueueDeclareOptions::default(),
                    FieldTable::default(),
                )
                .await
        })
        .map_err(|_| rquickjs::Error::new_from_js("AMQP Queue declare failed", "NetworkError"))?;

        // Start consuming
        let mut consumer = rt
            .block_on(async {
                channel
                    .basic_consume(
                        &queue,
                        "fusillade-consumer",
                        BasicConsumeOptions {
                            no_ack: false, // Manual ack
                            ..Default::default()
                        },
                        FieldTable::default(),
                    )
                    .await
            })
            .map_err(|_| rquickjs::Error::new_from_js("AMQP Consume failed", "NetworkError"))?;

        // Create channel for messages
        let (tx, rx) = crossbeam_channel::bounded::<AmqpMessage>(1000);

        // Set up running flag
        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let rt_clone = self.runtime.clone();

        // Spawn background thread to poll consumer stream
        let handle = std::thread::spawn(move || {
            rt_clone.block_on(async {
                while running.load(Ordering::SeqCst) {
                    // Use tokio timeout for async polling
                    let timeout_result = tokio::time::timeout(
                        tokio::time::Duration::from_millis(100),
                        consumer.next(),
                    )
                    .await;

                    match timeout_result {
                        Ok(Some(Ok(delivery))) => {
                            let msg = AmqpMessage {
                                body: delivery.data.clone(),
                                delivery_tag: delivery.delivery_tag,
                            };
                            // Send to channel, ignore if full or disconnected
                            if tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Ok(Some(Err(_))) => {
                            // Delivery error, continue
                        }
                        Ok(None) => {
                            // Consumer closed
                            break;
                        }
                        Err(_) => {
                            // Timeout, continue polling
                        }
                    }
                }
            });
        });

        self.message_rx = Some(rx);
        self._background_handle = Some(handle);

        Ok(())
    }

    /// Receive the next message from the queue
    /// Returns { body, deliveryTag } or null if no message/disconnected
    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        if let Some(ref rx) = self.message_rx {
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(msg) => {
                    let obj = Object::new(ctx.clone())?;
                    // Try to decode body as UTF-8 string
                    let body_str = String::from_utf8_lossy(&msg.body).into_owned();
                    obj.set("body", body_str)?;
                    obj.set("deliveryTag", msg.delivery_tag)?;
                    Ok(obj.into_value())
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(Value::new_null(ctx)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Ok(Value::new_null(ctx)),
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    /// Acknowledge a message by its delivery tag
    pub fn ack(&self, delivery_tag: u64) -> Result<()> {
        let channel = self.channel.as_ref().ok_or_else(|| {
            rquickjs::Error::new_from_js("AMQP Client not connected", "StateError")
        })?;

        let rt = self.runtime.clone();
        rt.block_on(async {
            channel
                .basic_ack(delivery_tag, BasicAckOptions::default())
                .await
        })
        .map_err(|_| rquickjs::Error::new_from_js("AMQP Ack failed", "NetworkError"))?;

        Ok(())
    }

    /// Negative acknowledge (reject) a message
    pub fn nack(&self, delivery_tag: u64, requeue: bool) -> Result<()> {
        let channel = self.channel.as_ref().ok_or_else(|| {
            rquickjs::Error::new_from_js("AMQP Client not connected", "StateError")
        })?;

        let rt = self.runtime.clone();
        rt.block_on(async {
            channel
                .basic_nack(
                    delivery_tag,
                    BasicNackOptions {
                        multiple: false,
                        requeue,
                    },
                )
                .await
        })
        .map_err(|_| rquickjs::Error::new_from_js("AMQP Nack failed", "NetworkError"))?;

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
        // Signal background thread to stop
        self.running.store(false, Ordering::SeqCst);

        self.connection = None;
        self.channel = None;
        self.message_rx = None;
        self._background_handle = None;
        Ok(())
    }
}

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let globals = ctx.globals();
    rquickjs::Class::<JsAmqpClient>::define(&globals)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_amqp_message_struct() {
        let msg = AmqpMessage {
            body: b"test message".to_vec(),
            delivery_tag: 12345,
        };

        assert_eq!(msg.body, b"test message");
        assert_eq!(msg.delivery_tag, 12345);
    }

    #[test]
    fn test_amqp_message_utf8_body() {
        let msg = AmqpMessage {
            body: r#"{"event":"order.created","data":{}}"#.as_bytes().to_vec(),
            delivery_tag: 1,
        };

        let body_str = String::from_utf8_lossy(&msg.body).into_owned();
        assert!(body_str.contains("order.created"));
    }

    #[test]
    fn test_amqp_delivery_tag_handling() {
        // Verify delivery tags are properly stored and retrieved
        let tags: Vec<u64> = vec![1, 100, 999999, u64::MAX];

        for tag in tags {
            let msg = AmqpMessage {
                body: vec![],
                delivery_tag: tag,
            };
            assert_eq!(msg.delivery_tag, tag);
        }
    }

    #[test]
    fn test_amqp_message_binary_body() {
        // Test that binary data is handled correctly
        let binary_data: Vec<u8> = vec![0x00, 0xFF, 0x7F, 0x80];
        let msg = AmqpMessage {
            body: binary_data.clone(),
            delivery_tag: 1,
        };

        assert_eq!(msg.body, binary_data);
    }
}
