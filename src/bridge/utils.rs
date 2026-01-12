use rquickjs::{Ctx, Function, Object, Result, Value, Array};
use uuid::Uuid;
use rand::{Rng, distributions::Alphanumeric};

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
        Uuid::new_v4().to_string()
    }))?;

    // randomInt(min, max) -> i32
    utils.set("randomInt", Function::new(ctx.clone(), |min: i32, max: i32| -> i32 {
        if min >= max { return min; }
        rand::thread_rng().gen_range(min..=max)
    }))?;

    // randomString(length) -> String
    utils.set("randomString", Function::new(ctx.clone(), |len: usize| -> String {
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(len)
            .map(char::from)
            .collect()
    }))?;

    // randomItem(Array) -> Value
    utils.set("randomItem", Function::new(ctx.clone(), random_item))?;

    ctx.globals().set("utils", utils)?;
    Ok(())
}
