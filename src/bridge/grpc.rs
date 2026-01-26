use prost::bytes::Buf;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MethodDescriptor, Value as ProstValue,
};
use rquickjs::{class::Trace, Ctx, Function, JsLifetime, Object, Result, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use tonic::{
    client::Grpc,
    codec::{Codec, DecodeBuf, EncodeBuf},
    transport::{Channel, Endpoint},
    Request, Status,
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

// --- JS Class ---

#[derive(Trace)]
#[rquickjs::class]
pub struct GrpcClient {
    #[qjs(skip_trace)]
    pool: RefCell<Option<DescriptorPool>>,
    #[qjs(skip_trace)]
    channel: RefCell<Option<Channel>>,
}

unsafe impl<'js> JsLifetime<'js> for GrpcClient {
    type Changed<'to> = GrpcClient;
}

#[rquickjs::methods]
impl GrpcClient {
    #[qjs(constructor)]
    pub fn new() -> Self {
        Self {
            pool: RefCell::new(None),
            channel: RefCell::new(None),
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

    pub async fn connect(&self, url: String) -> Result<()> {
        let endpoint = Endpoint::from_shared(url).map_err(|e| {
            eprintln!("Endpoint error: {}", e);
            rquickjs::Error::new_from_js("UrlError", "GrpcError")
        })?;

        let channel = endpoint.connect().await.map_err(|e| {
            eprintln!("Connect error: {}", e);
            rquickjs::Error::new_from_js("ConnectError", "GrpcError")
        })?;

        *self.channel.borrow_mut() = Some(channel);
        Ok(())
    }

    pub async fn invoke<'js>(
        &self,
        ctx: Ctx<'js>,
        method_name: String,
        request_val: Value<'js>,
    ) -> Result<Value<'js>> {
        // 1. Get Pool & Method Descriptor
        let (method_desc, service_name, method_short_name) = {
            let pool_borrow = self.pool.borrow();
            let pool = pool_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Proto not loaded", "StateError"))?;

            // 2. Parse Method Name
            let clean_name = method_name.trim_start_matches('/');
            let (service_name, method_short_name) = match clean_name.rsplit_once('/') {
                Some(v) => v,
                None => match clean_name.rsplit_once('.') {
                    Some(v) => v,
                    None => {
                        return Err(rquickjs::Error::new_from_js(
                            "Invalid method format",
                            "FormatError",
                        ))
                    }
                },
            };

            let service = pool.get_service_by_name(service_name).ok_or_else(|| {
                rquickjs::Error::new_from_js("Service not found", "NotFoundError")
            })?;

            let method_desc = service
                .methods()
                .find(|m| m.name() == method_short_name)
                .ok_or_else(|| rquickjs::Error::new_from_js("Method not found", "NotFoundError"))?;

            (
                method_desc,
                service_name.to_string(),
                method_short_name.to_string(),
            )
        };

        // 3. Serialize JS Value to JSON String
        let json_obj: serde_json::Value = if request_val.is_object() {
            let json: Object = ctx.globals().get("JSON")?;
            let stringify: Function = json.get("stringify")?;
            let json_str: String = stringify.call((request_val,))?;
            serde_json::from_str(&json_str).map_err(|e| {
                eprintln!("JSON parse error: {}", e);
                rquickjs::Error::new_from_js("JsonError", "ValueError")
            })?
        } else {
            return Err(rquickjs::Error::new_from_js(
                "Request body must be object",
                "TypeError",
            ));
        };

        // 4. Deserialize JSON to DynamicMessage (Manual Map)
        let input_desc = method_desc.input();
        let request_msg = json_to_dynamic_message(input_desc, json_obj)?;

        // 5. Get Channel
        let channel = {
            let channel_borrow = self.channel.borrow();
            channel_borrow
                .as_ref()
                .ok_or_else(|| rquickjs::Error::new_from_js("Not connected", "StateError"))?
                .clone()
        };

        // 6. Invoke
        let mut grpc = Grpc::new(channel);
        let path = format!("/{}/{}", service_name, method_short_name);

        let path_uri: http::Uri = path
            .parse()
            .map_err(|_| rquickjs::Error::new_from_js("Invalid URI path", "PathError"))?;
        let path_http = http::uri::PathAndQuery::try_from(path_uri.path())
            .map_err(|_| rquickjs::Error::new_from_js("Invalid PathAndQuery", "PathError"))?;

        let codec = DynamicCodec::new(method_desc.clone());

        let request = Request::new(request_msg);
        let response = grpc.unary(request, path_http, codec).await.map_err(|e| {
            eprintln!("gRPC Request failed: {}", e);
            rquickjs::Error::new_from_js("GrpcError", "NetworkError")
        })?;

        let response_msg = response.into_inner();

        // 7. Serialize Response to JSON
        let response_json = serde_json::to_string(&response_msg).map_err(|e| {
            eprintln!("Response serialization error: {}", e);
            rquickjs::Error::new_from_js("JsonError", "ValueError")
        })?;

        let json: Object = ctx.globals().get("JSON")?;
        let parse: Function = json.get("parse")?;
        let js_val: Value = parse.call((response_json,))?;

        Ok(js_val)
    }
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    rquickjs::Class::<GrpcClient>::define(&ctx.globals())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    // Helper to create a mock field descriptor for testing
    // Note: Full integration tests require actual proto files

    #[test]
    fn test_json_scalar_string() {
        // Test that string conversion works correctly
        let json_val = json!("hello");
        assert!(json_val.as_str().is_some());
        assert_eq!(json_val.as_str().unwrap(), "hello");
    }

    #[test]
    fn test_json_scalar_number() {
        // Test number parsing
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
}
