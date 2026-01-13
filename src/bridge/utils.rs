use rquickjs::{Ctx, Function, Object, Result, Value, Array};
use uuid::Uuid;
use rand::{Rng, distributions::Alphanumeric};

fn generate_uuid() -> String {
    Uuid::new_v4().to_string()
}

fn random_int(min: i32, max: i32) -> i32 {
    if min >= max { return min; }
    rand::thread_rng().gen_range(min..=max)
}

fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn random_item<'js>(_ctx: Ctx<'js>, arr: Array<'js>) -> Result<Option<Value<'js>>> {
    let len = arr.len();
    if len == 0 { return Ok(None); }
    let idx = rand::thread_rng().gen_range(0..len);
    arr.get(idx).map(Some)
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    let utils = Object::new(ctx.clone())?;

    // uuid() -> String
    utils.set("uuid", Function::new(ctx.clone(), || -> String {
        generate_uuid()
    }))?;

    // randomInt(min, max) -> i32
    utils.set("randomInt", Function::new(ctx.clone(), |min: i32, max: i32| -> i32 {
        random_int(min, max)
    }))?;

    // randomString(length) -> String
    utils.set("randomString", Function::new(ctx.clone(), |len: usize| -> String {
        random_string(len)
    }))?;

    // randomItem(Array) -> Value
    utils.set("randomItem", Function::new(ctx.clone(), random_item))?;

    ctx.globals().set("utils", utils)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_format() {
        let uuid = generate_uuid();
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.chars().filter(|&c| c == '-').count(), 4);
    }

    #[test]
    fn test_uuid_uniqueness() {
        let uuid1 = generate_uuid();
        let uuid2 = generate_uuid();
        assert_ne!(uuid1, uuid2);
    }

    #[test]
    fn test_random_int_range() {
        for _ in 0..100 {
            let val = random_int(1, 10);
            assert!(val >= 1 && val <= 10);
        }
    }

    #[test]
    fn test_random_int_same_bounds() {
        let val = random_int(5, 5);
        assert_eq!(val, 5);
    }

    #[test]
    fn test_random_int_min_greater_than_max() {
        let val = random_int(10, 5);
        assert_eq!(val, 10); // Returns min when min >= max
    }

    #[test]
    fn test_random_int_negative() {
        for _ in 0..100 {
            let val = random_int(-10, -1);
            assert!(val >= -10 && val <= -1);
        }
    }

    #[test]
    fn test_random_string_length() {
        assert_eq!(random_string(0).len(), 0);
        assert_eq!(random_string(10).len(), 10);
        assert_eq!(random_string(100).len(), 100);
    }

    #[test]
    fn test_random_string_alphanumeric() {
        let s = random_string(1000);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn test_random_string_uniqueness() {
        let s1 = random_string(32);
        let s2 = random_string(32);
        assert_ne!(s1, s2);
    }
}
