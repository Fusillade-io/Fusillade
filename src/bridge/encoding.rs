use rquickjs::{Ctx, Object, Result, Function};
use base64::{Engine as _, engine::general_purpose};

fn b64encode(data: &str) -> String {
    general_purpose::STANDARD.encode(data)
}

fn b64decode(data: &str) -> std::result::Result<String, &'static str> {
    let decoded = general_purpose::STANDARD.decode(data)
        .map_err(|_| "Base64 decode failed")?;
    String::from_utf8(decoded)
        .map_err(|_| "Invalid UTF-8 after decode")
}

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let encoding = Object::new(ctx.clone())?;

    encoding.set("b64encode", Function::new(ctx.clone(), move |data: String| -> String {
        b64encode(&data)
    }))?;

    encoding.set("b64decode", Function::new(ctx.clone(), move |data: String| -> Result<String> {
        b64decode(&data)
            .map_err(|e| rquickjs::Error::new_from_js(e, "ValueError"))
    }))?;

    ctx.globals().set("encoding", encoding)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_b64encode_basic() {
        assert_eq!(b64encode("hello"), "aGVsbG8=");
    }

    #[test]
    fn test_b64encode_empty() {
        assert_eq!(b64encode(""), "");
    }

    #[test]
    fn test_b64encode_unicode() {
        let encoded = b64encode("héllo wörld");
        assert!(!encoded.is_empty());
        // Verify roundtrip
        assert_eq!(b64decode(&encoded).unwrap(), "héllo wörld");
    }

    #[test]
    fn test_b64decode_basic() {
        assert_eq!(b64decode("aGVsbG8=").unwrap(), "hello");
    }

    #[test]
    fn test_b64decode_empty() {
        assert_eq!(b64decode("").unwrap(), "");
    }

    #[test]
    fn test_b64decode_invalid() {
        assert!(b64decode("!!!invalid!!!").is_err());
    }

    #[test]
    fn test_b64_roundtrip() {
        let original = "The quick brown fox jumps over the lazy dog";
        let encoded = b64encode(original);
        let decoded = b64decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_b64encode_binary_like() {
        // Test with bytes that might appear in binary data
        let data = "\x00\x01\x02\x7f";
        let encoded = b64encode(data);
        assert!(!encoded.is_empty());
    }
}
