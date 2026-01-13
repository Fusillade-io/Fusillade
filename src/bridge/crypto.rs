use rquickjs::{Ctx, Object, Result, Function};
use md5::Md5;
use sha1::Sha1;
use sha2::{Sha256, Digest};

fn hmac_md5(key: &[u8], data: &[u8]) -> String {
    // Simple HMAC implementation for MD5
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    
    if key.len() > BLOCK_SIZE {
        let mut hasher = Md5::new();
        hasher.update(key);
        let hash = hasher.finalize();
        key_block[..16].copy_from_slice(&hash);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    
    let i_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x36).collect();
    let o_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x5c).collect();
    
    let mut inner_hasher = Md5::new();
    inner_hasher.update(&i_key_pad);
    inner_hasher.update(data);
    let inner_hash = inner_hasher.finalize();
    
    let mut outer_hasher = Md5::new();
    outer_hasher.update(&o_key_pad);
    outer_hasher.update(inner_hash);
    format!("{:x}", outer_hasher.finalize())
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    
    if key.len() > BLOCK_SIZE {
        let mut hasher = Sha1::new();
        hasher.update(key);
        let hash = hasher.finalize();
        key_block[..20].copy_from_slice(&hash);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    
    let i_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x36).collect();
    let o_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x5c).collect();
    
    let mut inner_hasher = Sha1::new();
    inner_hasher.update(&i_key_pad);
    inner_hasher.update(data);
    let inner_hash = inner_hasher.finalize();
    
    let mut outer_hasher = Sha1::new();
    outer_hasher.update(&o_key_pad);
    outer_hasher.update(inner_hash);
    format!("{:x}", outer_hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> String {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0u8; BLOCK_SIZE];
    
    if key.len() > BLOCK_SIZE {
        let mut hasher = Sha256::new();
        hasher.update(key);
        let hash = hasher.finalize();
        key_block[..32].copy_from_slice(&hash);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    
    let i_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x36).collect();
    let o_key_pad: Vec<u8> = key_block.iter().map(|b| b ^ 0x5c).collect();
    
    let mut inner_hasher = Sha256::new();
    inner_hasher.update(&i_key_pad);
    inner_hasher.update(data);
    let inner_hash = inner_hasher.finalize();
    
    let mut outer_hasher = Sha256::new();
    outer_hasher.update(&o_key_pad);
    outer_hasher.update(inner_hash);
    format!("{:x}", outer_hasher.finalize())
}

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let crypto = Object::new(ctx.clone())?;

    crypto.set("md5", Function::new(ctx.clone(), move |data: String| -> String {
        let mut hasher = Md5::new();
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
    }))?;

    crypto.set("sha1", Function::new(ctx.clone(), move |data: String| -> String {
        let mut hasher = Sha1::new();
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
    }))?;

    crypto.set("sha256", Function::new(ctx.clone(), move |data: String| -> String {
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        format!("{:x}", hasher.finalize())
    }))?;

    crypto.set("hmac", Function::new(ctx.clone(), move |algorithm: String, key: String, data: String| -> String {
        match algorithm.as_str() {
            "md5" => hmac_md5(key.as_bytes(), data.as_bytes()),
            "sha1" => hmac_sha1(key.as_bytes(), data.as_bytes()),
            "sha256" => hmac_sha256(key.as_bytes(), data.as_bytes()),
            _ => "unsupported algorithm".to_string(),
        }
    }))?;

    ctx.globals().set("crypto", crypto)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use md5::Md5;
    use sha1::Sha1;
    use sha2::{Sha256, Digest};

    // --- HMAC Tests ---

    #[test]
    fn test_hmac_md5() {
        let result = hmac_md5(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(result.len(), 32);
        // Known test vector from RFC 2104
        assert_eq!(result, "80070713463e7749b90c2dc24911e275");
    }

    #[test]
    fn test_hmac_sha1() {
        let result = hmac_sha1(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(result.len(), 40);
        // Known test vector
        assert_eq!(result, "de7c9b85b8b78aa6bc8a7a36f70a90701c9db4d9");
    }

    #[test]
    fn test_hmac_sha256() {
        let result = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(result.len(), 64);
        // Known test vector
        assert_eq!(result, "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
    }

    #[test]
    fn test_hmac_consistency() {
        let result1 = hmac_sha256(b"secret", b"data");
        let result2 = hmac_sha256(b"secret", b"data");
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_hmac_different_keys() {
        let result1 = hmac_sha256(b"key1", b"data");
        let result2 = hmac_sha256(b"key2", b"data");
        assert_ne!(result1, result2);
    }

    #[test]
    fn test_hmac_different_data() {
        let result1 = hmac_sha256(b"key", b"data1");
        let result2 = hmac_sha256(b"key", b"data2");
        assert_ne!(result1, result2);
    }

    #[test]
    fn test_hmac_empty_data() {
        let result = hmac_sha256(b"key", b"");
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_hmac_empty_key() {
        let result = hmac_sha256(b"", b"data");
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_hmac_long_key() {
        // Key longer than block size (64 bytes) should be hashed first
        let long_key = b"this is a very long key that exceeds the block size of sixty four bytes and should be hashed";
        let result = hmac_sha256(long_key, b"test data");
        assert_eq!(result.len(), 64);
    }

    // --- Direct Hash Tests ---

    #[test]
    fn test_md5_known_vector() {
        let mut hasher = Md5::new();
        hasher.update(b"hello");
        let result = format!("{:x}", hasher.finalize());
        assert_eq!(result, "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn test_sha1_known_vector() {
        let mut hasher = Sha1::new();
        hasher.update(b"hello");
        let result = format!("{:x}", hasher.finalize());
        assert_eq!(result, "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    }

    #[test]
    fn test_sha256_known_vector() {
        let mut hasher = Sha256::new();
        hasher.update(b"hello");
        let result = format!("{:x}", hasher.finalize());
        assert_eq!(result, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn test_md5_empty() {
        let mut hasher = Md5::new();
        hasher.update(b"");
        let result = format!("{:x}", hasher.finalize());
        assert_eq!(result, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn test_sha256_empty() {
        let mut hasher = Sha256::new();
        hasher.update(b"");
        let result = format!("{:x}", hasher.finalize());
        assert_eq!(result, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }
}
