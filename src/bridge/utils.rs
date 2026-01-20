use rquickjs::{Ctx, Function, Object, Result, Value, Array, IntoJs};
use uuid::Uuid;
use rand::{Rng, distributions::Alphanumeric};
use std::sync::atomic::{AtomicU64, Ordering};

// Global counter for sequential IDs (thread-safe)
static SEQUENTIAL_COUNTER: AtomicU64 = AtomicU64::new(0);

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

// Data generation utilities

fn random_email() -> String {
    let user = random_string(8).to_lowercase();
    let domains = ["test.com", "example.com", "mail.test", "localhost.local"];
    let domain = domains[rand::thread_rng().gen_range(0..domains.len())];
    format!("{}@{}", user, domain)
}

fn random_phone() -> String {
    let mut rng = rand::thread_rng();
    format!("+1-{:03}-{:03}-{:04}",
        rng.gen_range(200..999),
        rng.gen_range(200..999),
        rng.gen_range(1000..9999)
    )
}

#[derive(Debug)]
struct RandomName {
    first: String,
    last: String,
    full: String,
}

impl<'js> IntoJs<'js> for RandomName {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("first", self.first.clone())?;
        obj.set("last", self.last.clone())?;
        obj.set("full", self.full)?;
        Ok(obj.into_value())
    }
}

fn random_name() -> RandomName {
    let first_names = ["James", "Mary", "John", "Patricia", "Robert", "Jennifer", "Michael", "Linda",
                       "William", "Elizabeth", "David", "Susan", "Richard", "Jessica", "Joseph", "Sarah"];
    let last_names = ["Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
                      "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson", "Thomas"];

    let mut rng = rand::thread_rng();
    let first = first_names[rng.gen_range(0..first_names.len())].to_string();
    let last = last_names[rng.gen_range(0..last_names.len())].to_string();
    let full = format!("{} {}", first, last);

    RandomName { first, last, full }
}

fn random_date(start_year: i32, end_year: i32) -> String {
    let mut rng = rand::thread_rng();
    let year = rng.gen_range(start_year..=end_year);
    let month = rng.gen_range(1..=12);
    let max_day = match month {
        2 => if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) { 29 } else { 28 },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let day = rng.gen_range(1..=max_day);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn sequential_id(worker_id: usize) -> u64 {
    // Each worker gets a unique range: worker_id * 1_000_000_000 + counter
    // This ensures IDs are unique across workers
    let base = (worker_id as u64) * 1_000_000_000;
    let counter = SEQUENTIAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    base + counter
}

pub fn register_sync(ctx: &Ctx, worker_id: usize) -> Result<()> {
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

    // randomEmail() -> String
    utils.set("randomEmail", Function::new(ctx.clone(), || -> String {
        random_email()
    }))?;

    // randomPhone() -> String
    utils.set("randomPhone", Function::new(ctx.clone(), || -> String {
        random_phone()
    }))?;

    // randomName() -> { first, last, full }
    utils.set("randomName", Function::new(ctx.clone(), || -> RandomName {
        random_name()
    }))?;

    // randomDate(startYear, endYear) -> String (YYYY-MM-DD)
    utils.set("randomDate", Function::new(ctx.clone(), |start: i32, end: i32| -> String {
        random_date(start, end)
    }))?;

    // sequentialId() -> u64 (unique across workers)
    utils.set("sequentialId", Function::new(ctx.clone(), move || -> u64 {
        sequential_id(worker_id)
    }))?;

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
            assert!((1..=10).contains(&val));
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
            assert!((-10..=-1).contains(&val));
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

    #[test]
    fn test_random_email_format() {
        let email = random_email();
        assert!(email.contains('@'));
        let parts: Vec<&str> = email.split('@').collect();
        assert_eq!(parts.len(), 2);
        assert!(!parts[0].is_empty());
        assert!(!parts[1].is_empty());
    }

    #[test]
    fn test_random_phone_format() {
        let phone = random_phone();
        assert!(phone.starts_with("+1-"));
        assert_eq!(phone.len(), 15); // +1-XXX-XXX-XXXX
    }

    #[test]
    fn test_random_name_fields() {
        let name = random_name();
        assert!(!name.first.is_empty());
        assert!(!name.last.is_empty());
        assert!(name.full.contains(' '));
        assert!(name.full.contains(&name.first));
        assert!(name.full.contains(&name.last));
    }

    #[test]
    fn test_random_date_format() {
        let date = random_date(2020, 2025);
        assert_eq!(date.len(), 10); // YYYY-MM-DD
        let parts: Vec<&str> = date.split('-').collect();
        assert_eq!(parts.len(), 3);
        let year: i32 = parts[0].parse().unwrap();
        let month: i32 = parts[1].parse().unwrap();
        let day: i32 = parts[2].parse().unwrap();
        assert!((2020..=2025).contains(&year));
        assert!((1..=12).contains(&month));
        assert!((1..=31).contains(&day));
    }

    #[test]
    fn test_sequential_id_uniqueness() {
        let id1 = sequential_id(0);
        let id2 = sequential_id(0);
        let id3 = sequential_id(0);
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert!(id2 > id1);
        assert!(id3 > id2);
    }

    #[test]
    fn test_sequential_id_worker_separation() {
        // Different workers should have different base ranges
        let id_w0 = sequential_id(0);
        let id_w1 = sequential_id(1);
        // Worker 1's base is 1_000_000_000, so id_w1 should be much larger
        assert!(id_w1 > id_w0 + 999_000_000);
    }
}
