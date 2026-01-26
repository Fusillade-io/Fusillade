use futures_lite::StreamExt;
use prost::bytes::Buf;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MethodDescriptor, Value as ProstValue,
};
use rquickjs::{class::Trace, Ctx, Function, JsLifetime, Object, Result, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tonic::{
    client::Grpc,
    codec::{Codec, DecodeBuf, EncodeBuf},
    transport::{Channel, Endpoint},
    Request, Status, Streaming,
};

// --- Helper: JSON -> DynamicMessage ---

fn json_to_dynamic_message(
    desc: prost_reflect::MessageDescriptor,
    json: serde_json::Value,
) -> Result<DynamicMessage> {
    let mut msg = DynamicMessage::new(desc.clone());

    if let serde_json::Value::Object(map) = json {
        for (key, val) in map {
            if let Some(field) = desc.get_field_by_name(&key) {
                let prost_val = json_val_to_prost(val, &field)?;
                msg.set_field(&field, prost_val);
            }
        }
    } else {
        return Err(rquickjs::Error::new_from_js(
            "Expected JSON object for message",
            "TypeError",
        ));
    }

    Ok(msg)
}

fn json_val_to_prost(json: serde_json::Value, field: &FieldDescriptor) -> Result<ProstValue> {
    // Handle repeated fields (arrays)
    if field.is_list() {
        let arr = json.as_array().ok_or_else(|| {
            rquickjs::Error::new_from_js("Expected array for repeated field", "TypeError")
        })?;
        let items: Result<Vec<ProstValue>> = arr
            .iter()
            .map(|item| json_scalar_to_prost(item.clone(), field))
            .collect();
        return Ok(ProstValue::List(items?));
    }

    // Handle map fields
    if field.is_map() {
        let obj = json.as_object().ok_or_else(|| {
            rquickjs::Error::new_from_js("Expected object for map field", "TypeError")
        })?;
        let map_entry = field.kind();
        if let Kind::Message(entry_desc) = map_entry {
            let key_field = entry_desc
                .get_field_by_name("key")
                .ok_or_else(|| rquickjs::Error::new_from_js("Invalid map entry", "TypeError"))?;
            let value_field = entry_desc
                .get_field_by_name("value")
                .ok_or_else(|| rquickjs::Error::new_from_js("Invalid map entry", "TypeError"))?;

            let mut map = std::collections::HashMap::new();
            for (k, v) in obj {
                let key_val =
                    json_scalar_to_prost(serde_json::Value::String(k.clone()), &key_field)?;
                let val_val = json_scalar_to_prost(v.clone(), &value_field)?;
                if let ProstValue::String(key_str) = key_val {
                    map.insert(prost_reflect::MapKey::String(key_str), val_val);
                } else if let ProstValue::I32(key_i32) = key_val {
                    map.insert(prost_reflect::MapKey::I32(key_i32), val_val);
                } else if let ProstValue::I64(key_i64) = key_val {
                    map.insert(prost_reflect::MapKey::I64(key_i64), val_val);
                } else if let ProstValue::U32(key_u32) = key_val {
                    map.insert(prost_reflect::MapKey::U32(key_u32), val_val);
                } else if let ProstValue::U64(key_u64) = key_val {
                    map.insert(prost_reflect::MapKey::U64(key_u64), val_val);
                } else if let ProstValue::Bool(key_bool) = key_val {
                    map.insert(prost_reflect::MapKey::Bool(key_bool), val_val);
                }
            }
            return Ok(ProstValue::Map(map));
        }
        return Err(rquickjs::Error::new_from_js(
            "Invalid map field type",
            "TypeError",
        ));
    }

    // Handle scalar types
    json_scalar_to_prost(json, field)
}

/// Convert a JSON value to a prost scalar value
fn json_scalar_to_prost(json: serde_json::Value, field: &FieldDescriptor) -> Result<ProstValue> {
    match field.kind() {
        Kind::Double => json
            .as_f64()
            .map(ProstValue::F64)
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected double", "TypeError")),
        Kind::Float => json
            .as_f64()
            .map(|f| ProstValue::F32(f as f32))
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected float", "TypeError")),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => json
            .as_i64()
            .map(|i| ProstValue::I32(i as i32))
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected int32", "TypeError")),
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => json
            .as_i64()
            .map(ProstValue::I64)
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected int64", "TypeError")),
        Kind::Uint32 | Kind::Fixed32 => json
            .as_u64()
            .map(|i| ProstValue::U32(i as u32))
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected uint32", "TypeError")),
        Kind::Uint64 | Kind::Fixed64 => json
            .as_u64()
            .map(ProstValue::U64)
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected uint64", "TypeError")),
        Kind::Bool => json
            .as_bool()
            .map(ProstValue::Bool)
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected bool", "TypeError")),
        Kind::String => json
            .as_str()
            .map(|s| ProstValue::String(s.to_string()))
            .ok_or_else(|| rquickjs::Error::new_from_js("Expected string", "TypeError")),
        Kind::Bytes => {
            // Expect base64-encoded string
            let b64_str = json.as_str().ok_or_else(|| {
                rquickjs::Error::new_from_js("Expected base64 string for bytes", "TypeError")
            })?;
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(b64_str)
                .map_err(|_| {
                    rquickjs::Error::new_from_js("Invalid base64 encoding", "TypeError")
                })?;
            Ok(ProstValue::Bytes(prost::bytes::Bytes::from(decoded)))
        }
        Kind::Enum(enum_desc) => {
            // Accept either string name or integer value
            if let Some(name) = json.as_str() {
                enum_desc
                    .get_value_by_name(name)
                    .map(|v| ProstValue::EnumNumber(v.number()))
                    .ok_or_else(|| rquickjs::Error::new_from_js("Unknown enum value", "TypeError"))
            } else if let Some(num) = json.as_i64() {
                Ok(ProstValue::EnumNumber(num as i32))
            } else {
                Err(rquickjs::Error::new_from_js(
                    "Expected enum string or number",
                    "TypeError",
                ))
            }
        }
        Kind::Message(msg_desc) => {
            let nested_msg = json_to_dynamic_message(msg_desc, json)?;
            Ok(ProstValue::Message(nested_msg))
        }
    }
}

/// Convert a DynamicMessage to JS Value
fn dynamic_message_to_js<'js>(ctx: &Ctx<'js>, msg: &DynamicMessage) -> Result<Value<'js>> {
    let response_json = serde_json::to_string(msg).map_err(|e| {
        eprintln!("Response serialization error: {}", e);
        rquickjs::Error::new_from_js("JsonError", "ValueError")
    })?;

    let json: Object = ctx.globals().get("JSON")?;
    let parse: Function = json.get("parse")?;
    parse.call((response_json,))
}

/// Convert JS Value to serde_json::Value
fn js_to_json<'js>(ctx: &Ctx<'js>, val: Value<'js>) -> Result<serde_json::Value> {
    if val.is_object() {
        let json: Object = ctx.globals().get("JSON")?;
        let stringify: Function = json.get("stringify")?;
        let json_str: String = stringify.call((val,))?;
        serde_json::from_str(&json_str).map_err(|e| {
            eprintln!("JSON parse error: {}", e);
            rquickjs::Error::new_from_js("JsonError", "ValueError")
        })
    } else {
        Err(rquickjs::Error::new_from_js(
            "Request body must be object",
            "TypeError",
        ))
    }
}

// --- Dynamic Codec for Tonic ---

#[derive(Debug, Clone)]
struct DynamicCodec {
    method: MethodDescriptor,
}

impl DynamicCodec {
    fn new(method: MethodDescriptor) -> Self {
        Self { method }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;

    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder {
            method: self.method.clone(),
        }
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            method: self.method.clone(),
        }
    }
}

struct DynamicEncoder {
    #[allow(dead_code)]
    method: MethodDescriptor,
}

impl tonic::codec::Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(
        &mut self,
        item: Self::Item,
        buf: &mut EncodeBuf<'_>,
    ) -> std::result::Result<(), Self::Error> {
        use prost::Message;
        item.encode(buf)
            .map_err(|e| Status::internal(e.to_string()))
    }
}

struct DynamicDecoder {
    method: MethodDescriptor,
}

impl tonic::codec::Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(
        &mut self,
        buf: &mut DecodeBuf<'_>,
    ) -> std::result::Result<Option<Self::Item>, Self::Error> {
        let input_desc = self.method.output();
        let mut msg = DynamicMessage::new(input_desc);
        use prost::Message;
        if buf.remaining() == 0 {
            return Ok(None);
        }
        msg.merge(buf)
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Some(msg))
    }
}

// --- Server Stream Class ---

/// Handle for server streaming RPC
#[derive(Trace)]
#[rquickjs::class]
pub struct GrpcServerStream {
    #[qjs(skip_trace)]
    message_rx: RefCell<Option<crossbeam_channel::Receiver<DynamicMessage>>>,
    #[qjs(skip_trace)]
    running: Arc<AtomicBool>,
}

unsafe impl<'js> JsLifetime<'js> for GrpcServerStream {
    type Changed<'to> = GrpcServerStream;
}

#[rquickjs::methods]
impl GrpcServerStream {
    /// Receive the next message from the server stream
    /// Returns message object or null if stream ended
    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let borrow = self.message_rx.borrow();
        if let Some(ref rx) = *borrow {
            match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(msg) => dynamic_message_to_js(&ctx, &msg),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(Value::new_null(ctx)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Ok(Value::new_null(ctx)),
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    /// Close the stream early
    pub fn close(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        *self.message_rx.borrow_mut() = None;
        Ok(())
    }
}

// --- Client Stream Class ---

/// Handle for client streaming RPC
#[derive(Trace)]
#[rquickjs::class]
pub struct GrpcClientStream {
    #[qjs(skip_trace)]
    message_tx: RefCell<Option<mpsc::Sender<DynamicMessage>>>,
    #[qjs(skip_trace)]
    response_rx: RefCell<
        Option<tokio::sync::oneshot::Receiver<std::result::Result<DynamicMessage, Status>>>,
    >,
    #[qjs(skip_trace)]
    runtime: Arc<Runtime>,
    #[qjs(skip_trace)]
    input_desc: prost_reflect::MessageDescriptor,
}

unsafe impl<'js> JsLifetime<'js> for GrpcClientStream {
    type Changed<'to> = GrpcClientStream;
}

#[rquickjs::methods]
impl GrpcClientStream {
    /// Send a message to the server
    pub fn send<'js>(&self, ctx: Ctx<'js>, msg_val: Value<'js>) -> Result<()> {
        let borrow = self.message_tx.borrow();
        if let Some(ref tx) = *borrow {
            let json_obj = js_to_json(&ctx, msg_val)?;
            let msg = json_to_dynamic_message(self.input_desc.clone(), json_obj)?;

            self.runtime
                .block_on(async { tx.send(msg).await })
                .map_err(|_| rquickjs::Error::new_from_js("Stream send failed", "NetworkError"))?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js(
                "Stream already closed",
                "StateError",
            ))
        }
    }

    /// Close the send side and wait for the server's response
    #[qjs(rename = "closeAndRecv")]
    pub fn close_and_recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // Drop the sender to signal end of stream
        *self.message_tx.borrow_mut() = None;

        // Wait for response
        let mut response_borrow = self.response_rx.borrow_mut();
        if let Some(rx) = response_borrow.take() {
            let result = self.runtime.block_on(rx);
            match result {
                Ok(Ok(msg)) => dynamic_message_to_js(&ctx, &msg),
                Ok(Err(e)) => {
                    eprintln!("gRPC error: {}", e);
                    Err(rquickjs::Error::new_from_js("GrpcError", "NetworkError"))
                }
                Err(_) => Err(rquickjs::Error::new_from_js(
                    "Response channel closed",
                    "NetworkError",
                )),
            }
        } else {
            Err(rquickjs::Error::new_from_js(
                "Already received response",
                "StateError",
            ))
        }
    }
}

// --- Bidi Stream Class ---

/// Handle for bidirectional streaming RPC
#[derive(Trace)]
#[rquickjs::class]
pub struct GrpcBidiStream {
    #[qjs(skip_trace)]
    message_tx: RefCell<Option<mpsc::Sender<DynamicMessage>>>,
    #[qjs(skip_trace)]
    message_rx: RefCell<Option<crossbeam_channel::Receiver<DynamicMessage>>>,
    #[qjs(skip_trace)]
    runtime: Arc<Runtime>,
    #[qjs(skip_trace)]
    running: Arc<AtomicBool>,
    #[qjs(skip_trace)]
    input_desc: prost_reflect::MessageDescriptor,
}

unsafe impl<'js> JsLifetime<'js> for GrpcBidiStream {
    type Changed<'to> = GrpcBidiStream;
}

#[rquickjs::methods]
impl GrpcBidiStream {
    /// Send a message to the server
    pub fn send<'js>(&self, ctx: Ctx<'js>, msg_val: Value<'js>) -> Result<()> {
        let borrow = self.message_tx.borrow();
        if let Some(ref tx) = *borrow {
            let json_obj = js_to_json(&ctx, msg_val)?;
            let msg = json_to_dynamic_message(self.input_desc.clone(), json_obj)?;

            self.runtime
                .block_on(async { tx.send(msg).await })
                .map_err(|_| rquickjs::Error::new_from_js("Stream send failed", "NetworkError"))?;
            Ok(())
        } else {
            Err(rquickjs::Error::new_from_js(
                "Stream already closed",
                "StateError",
            ))
        }
    }

    /// Receive a message from the server
    pub fn recv<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let borrow = self.message_rx.borrow();
        if let Some(ref rx) = *borrow {
            match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                Ok(msg) => dynamic_message_to_js(&ctx, &msg),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(Value::new_null(ctx)),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Ok(Value::new_null(ctx)),
            }
        } else {
            Ok(Value::new_null(ctx))
        }
    }

    /// Close the stream
    pub fn close(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        *self.message_tx.borrow_mut() = None;
        *self.message_rx.borrow_mut() = None;
        Ok(())
    }
}

// --- JS Class ---

#[derive(Trace)]
#[rquickjs::class]
pub struct GrpcClient {
    #[qjs(skip_trace)]
    pool: RefCell<Option<DescriptorPool>>,
    #[qjs(skip_trace)]
    channel: RefCell<Option<Channel>>,
    #[qjs(skip_trace)]
    runtime: Arc<Runtime>,
}

unsafe impl<'js> JsLifetime<'js> for GrpcClient {
    type Changed<'to> = GrpcClient;
}

/// Parse method name into (service_name, method_name)
fn parse_method_name(method_name: &str) -> Result<(&str, &str)> {
    let clean_name = method_name.trim_start_matches('/');
    match clean_name.rsplit_once('/') {
        Some(v) => Ok(v),
        None => match clean_name.rsplit_once('.') {
            Some(v) => Ok(v),
            None => Err(rquickjs::Error::new_from_js(
                "Invalid method format",
                "FormatError",
            )),
        },
    }
}

// Internal helper methods for GrpcClient
impl GrpcClient {
    /// Get method descriptor from pool
    fn get_method(&self, method_name: &str) -> Result<(MethodDescriptor, String, String)> {
        let pool_borrow = self.pool.borrow();
        let pool = pool_borrow
            .as_ref()
            .ok_or_else(|| rquickjs::Error::new_from_js("Proto not loaded", "StateError"))?;

        let (service_name, method_short_name) = parse_method_name(method_name)?;

        let service = pool
            .get_service_by_name(service_name)
            .ok_or_else(|| rquickjs::Error::new_from_js("Service not found", "NotFoundError"))?;

        let method_desc = service
            .methods()
            .find(|m| m.name() == method_short_name)
            .ok_or_else(|| rquickjs::Error::new_from_js("Method not found", "NotFoundError"))?;

        Ok((
            method_desc,
            service_name.to_string(),
            method_short_name.to_string(),
        ))
    }
}

#[rquickjs::methods]
impl GrpcClient {
    #[qjs(constructor)]
    pub fn new() -> Self {
        let rt = Runtime::new().expect("Failed to create gRPC runtime");
        Self {
            pool: RefCell::new(None),
            channel: RefCell::new(None),
            runtime: Arc::new(rt),
        }
    }

    pub fn load(&self, files: Vec<String>, includes: Vec<String>) -> Result<()> {
        let file_descriptors = protox::compile(
            files.iter().map(PathBuf::from),
            includes.iter().map(PathBuf::from),
        )
        .map_err(|e| {
            eprintln!("Proto compile error: {}", e);
            rquickjs::Error::new_from_js("ProtoCompileError", "GrpcError")
        })?;

        let pool = DescriptorPool::from_file_descriptor_set(file_descriptors).map_err(|e| {
            eprintln!("Descriptor pool error: {}", e);
            rquickjs::Error::new_from_js("DescriptorError", "GrpcError")
        })?;

        *self.pool.borrow_mut() = Some(pool);
        Ok(())
    }

    pub fn connect(&self, url: String) -> Result<()> {
        let rt = self.runtime.clone();

        let endpoint = Endpoint::from_shared(url).map_err(|e| {
            eprintln!("Endpoint error: {}", e);
            rquickjs::Error::new_from_js("UrlError", "GrpcError")
        })?;

        let channel = rt
            .block_on(async { endpoint.connect().await })
            .map_err(|e| {
                eprintln!("Connect error: {}", e);
                rquickjs::Error::new_from_js("ConnectError", "GrpcError")
            })?;

        *self.channel.borrow_mut() = Some(channel);
        Ok(())
    }

    pub fn invoke<'js>(
        &self,
        ctx: Ctx<'js>,
        method_name: String,
        request_val: Value<'js>,
    ) -> Result<Value<'js>> {
        let (method_desc, service_name, method_short_name) = self.get_method(&method_name)?;
        let json_obj = js_to_json(&ctx, request_val)?;

        let input_desc = method_desc.input();
        let request_msg = json_to_dynamic_message(input_desc, json_obj)?;

        let channel = {
            let channel_borrow = self.channel.borrow();
            channel_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Not connected", "StateError"))?
                .clone()
        };

        let path = format!("/{}/{}", service_name, method_short_name);
        let path_uri: http::Uri = path
            .parse()
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URI path", "PathError"))?;
        let path_http = http::uri::PathAndQuery::try_from(path_uri.path())
            .map_err(|_| rquickjs::Error::new_from_js("Invalid PathAndQuery", "PathError"))?;

        let codec = DynamicCodec::new(method_desc.clone());
        let request = Request::new(request_msg);

        let rt = self.runtime.clone();
        let response = rt
            .block_on(async {
                let mut grpc = Grpc::new(channel);
                grpc.unary(request, path_http, codec).await
            })
            .map_err(|e| {
                eprintln!("gRPC Request failed: {}", e);
                rquickjs::Error::new_from_js("GrpcError", "NetworkError")
            })?;

        let response_msg = response.into_inner();
        dynamic_message_to_js(&ctx, &response_msg)
    }

    /// Start a server streaming RPC
    #[qjs(rename = "serverStream")]
    pub fn server_stream<'js>(
        &self,
        ctx: Ctx<'js>,
        method_name: String,
        request_val: Value<'js>,
    ) -> Result<Value<'js>> {
        let (method_desc, service_name, method_short_name) = self.get_method(&method_name)?;
        let json_obj = js_to_json(&ctx, request_val)?;

        let input_desc = method_desc.input();
        let request_msg = json_to_dynamic_message(input_desc, json_obj)?;

        let channel = {
            let channel_borrow = self.channel.borrow();
            channel_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Not connected", "StateError"))?
                .clone()
        };

        let path = format!("/{}/{}", service_name, method_short_name);
        let path_uri: http::Uri = path
            .parse()
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URI path", "PathError"))?;
        let path_http = http::uri::PathAndQuery::try_from(path_uri.path())
            .map_err(|_| rquickjs::Error::new_from_js("Invalid PathAndQuery", "PathError"))?;

        let codec = DynamicCodec::new(method_desc.clone());
        let request = Request::new(request_msg);

        let rt = self.runtime.clone();

        // Start the server streaming call
        let mut streaming: Streaming<DynamicMessage> = rt
            .block_on(async {
                let mut grpc = Grpc::new(channel);
                grpc.server_streaming(request, path_http, codec).await
            })
            .map_err(|e| {
                eprintln!("gRPC Server stream failed: {}", e);
                rquickjs::Error::new_from_js("GrpcError", "NetworkError")
            })?
            .into_inner();

        // Create channel for messages
        let (tx, rx) = crossbeam_channel::bounded::<DynamicMessage>(1000);
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let rt_clone = self.runtime.clone();

        // Spawn background task to read from stream
        std::thread::spawn(move || {
            rt_clone.block_on(async {
                while running_clone.load(Ordering::SeqCst) {
                    let timeout_result = tokio::time::timeout(
                        tokio::time::Duration::from_millis(100),
                        streaming.next(),
                    )
                    .await;

                    match timeout_result {
                        Ok(Some(Ok(msg))) => {
                            if tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Ok(Some(Err(_))) => {
                            // Stream error
                            break;
                        }
                        Ok(None) => {
                            // Stream ended
                            break;
                        }
                        Err(_) => {
                            // Timeout, continue
                        }
                    }
                }
            });
        });

        let stream = GrpcServerStream {
            message_rx: RefCell::new(Some(rx)),
            running,
        };

        let instance = rquickjs::Class::<GrpcServerStream>::instance(ctx, stream)?;
        Ok(instance.into_value())
    }

    /// Start a client streaming RPC
    #[qjs(rename = "clientStream")]
    pub fn client_stream<'js>(&self, ctx: Ctx<'js>, method_name: String) -> Result<Value<'js>> {
        let (method_desc, service_name, method_short_name) = self.get_method(&method_name)?;

        let channel = {
            let channel_borrow = self.channel.borrow();
            channel_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Not connected", "StateError"))?
                .clone()
        };

        let path = format!("/{}/{}", service_name, method_short_name);
        let path_uri: http::Uri = path
            .parse()
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URI path", "PathError"))?;
        let path_http = http::uri::PathAndQuery::try_from(path_uri.path())
            .map_err(|_| rquickjs::Error::new_from_js("Invalid PathAndQuery", "PathError"))?;

        let codec = DynamicCodec::new(method_desc.clone());
        let input_desc = method_desc.input();

        // Create channel for sending messages
        let (tx, mut rx) = mpsc::channel::<DynamicMessage>(100);

        // Create oneshot for response
        let (response_tx, response_rx) =
            tokio::sync::oneshot::channel::<std::result::Result<DynamicMessage, Status>>();

        let rt = self.runtime.clone();

        // Spawn background task to handle the client streaming call
        std::thread::spawn(move || {
            rt.block_on(async {
                let request_stream = async_stream::stream! {
                    while let Some(msg) = rx.recv().await {
                        yield msg;
                    }
                };

                let mut grpc = Grpc::new(channel);
                let result = grpc
                    .client_streaming(Request::new(request_stream), path_http, codec)
                    .await;

                let response = match result {
                    Ok(resp) => Ok(resp.into_inner()),
                    Err(e) => Err(e),
                };

                let _ = response_tx.send(response);
            });
        });

        let stream = GrpcClientStream {
            message_tx: RefCell::new(Some(tx)),
            response_rx: RefCell::new(Some(response_rx)),
            runtime: self.runtime.clone(),
            input_desc,
        };

        let instance = rquickjs::Class::<GrpcClientStream>::instance(ctx, stream)?;
        Ok(instance.into_value())
    }

    /// Start a bidirectional streaming RPC
    #[qjs(rename = "bidiStream")]
    pub fn bidi_stream<'js>(&self, ctx: Ctx<'js>, method_name: String) -> Result<Value<'js>> {
        let (method_desc, service_name, method_short_name) = self.get_method(&method_name)?;

        let channel = {
            let channel_borrow = self.channel.borrow();
            channel_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Not connected", "StateError"))?
                .clone()
        };

        let path = format!("/{}/{}", service_name, method_short_name);
        let path_uri: http::Uri = path
            .parse()
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URI path", "PathError"))?;
        let path_http = http::uri::PathAndQuery::try_from(path_uri.path())
            .map_err(|_| rquickjs::Error::new_from_js("Invalid PathAndQuery", "PathError"))?;

        let codec = DynamicCodec::new(method_desc.clone());
        let input_desc = method_desc.input();

        // Create channel for sending messages
        let (tx, mut send_rx) = mpsc::channel::<DynamicMessage>(100);

        // Create channel for receiving messages
        let (recv_tx, recv_rx) = crossbeam_channel::bounded::<DynamicMessage>(1000);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();
        let rt = self.runtime.clone();

        // Spawn background task to handle the bidi streaming call
        std::thread::spawn(move || {
            rt.block_on(async {
                let request_stream = async_stream::stream! {
                    while let Some(msg) = send_rx.recv().await {
                        yield msg;
                    }
                };

                let mut grpc = Grpc::new(channel);
                let result = grpc
                    .streaming(Request::new(request_stream), path_http, codec)
                    .await;

                if let Ok(response) = result {
                    let mut streaming = response.into_inner();
                    while running_clone.load(Ordering::SeqCst) {
                        let timeout_result = tokio::time::timeout(
                            tokio::time::Duration::from_millis(100),
                            streaming.next(),
                        )
                        .await;

                        match timeout_result {
                            Ok(Some(Ok(msg))) => {
                                if recv_tx.send(msg).is_err() {
                                    break;
                                }
                            }
                            Ok(Some(Err(_))) => break,
                            Ok(None) => break,
                            Err(_) => continue,
                        }
                    }
                }
            });
        });

        let stream = GrpcBidiStream {
            message_tx: RefCell::new(Some(tx)),
            message_rx: RefCell::new(Some(recv_rx)),
            runtime: self.runtime.clone(),
            running,
            input_desc,
        };

        let instance = rquickjs::Class::<GrpcBidiStream>::instance(ctx, stream)?;
        Ok(instance.into_value())
    }
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    rquickjs::Class::<GrpcClient>::define(&ctx.globals())?;
    rquickjs::Class::<GrpcServerStream>::define(&ctx.globals())?;
    rquickjs::Class::<GrpcClientStream>::define(&ctx.globals())?;
    rquickjs::Class::<GrpcBidiStream>::define(&ctx.globals())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_json_scalar_string() {
        let json_val = json!("hello");
        assert!(json_val.as_str().is_some());
        assert_eq!(json_val.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_json_scalar_number() {
        let json_int = json!(42);
        let json_float = json!(2.5);

        assert_eq!(json_int.as_i64(), Some(42));
        assert_eq!(json_float.as_f64(), Some(2.5));
    }

    #[test]
    fn test_json_scalar_bool() {
        let json_true = json!(true);
        let json_false = json!(false);

        assert_eq!(json_true.as_bool(), Some(true));
        assert_eq!(json_false.as_bool(), Some(false));
    }

    #[test]
    fn test_json_array_parsing() {
        let json_arr = json!([1, 2, 3]);
        let arr = json_arr.as_array().unwrap();

        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert_eq!(arr[1].as_i64(), Some(2));
        assert_eq!(arr[2].as_i64(), Some(3));
    }

    #[test]
    fn test_json_map_parsing() {
        let json_map = json!({"key1": "value1", "key2": "value2"});
        let obj = json_map.as_object().unwrap();

        assert_eq!(obj.len(), 2);
        assert_eq!(obj.get("key1").unwrap().as_str(), Some("value1"));
        assert_eq!(obj.get("key2").unwrap().as_str(), Some("value2"));
    }

    #[test]
    fn test_base64_decoding() {
        use base64::Engine;
        let original = b"hello world";
        let encoded = base64::engine::general_purpose::STANDARD.encode(original);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn test_json_nested_object() {
        let json_nested = json!({
            "outer": {
                "inner": "value"
            }
        });

        let outer = json_nested.get("outer").unwrap();
        let inner = outer.get("inner").unwrap();
        assert_eq!(inner.as_str(), Some("value"));
    }

    #[test]
    fn test_parse_method_name_slash() {
        let result = parse_method_name("pkg.Service/Method").unwrap();
        assert_eq!(result, ("pkg.Service", "Method"));
    }

    #[test]
    fn test_parse_method_name_leading_slash() {
        let result = parse_method_name("/pkg.Service/Method").unwrap();
        assert_eq!(result, ("pkg.Service", "Method"));
    }

    #[test]
    fn test_parse_method_name_dot() {
        let result = parse_method_name("pkg.Service.Method").unwrap();
        assert_eq!(result, ("pkg.Service", "Method"));
    }

}
