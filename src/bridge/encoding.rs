use rquickjs::{Ctx, Object, Result, Function};
use base64::{Engine as _, engine::general_purpose};

pub fn register_sync<'js>(ctx: &Ctx<'js>) -> Result<()> {
    let encoding = Object::new(ctx.clone())?;

    encoding.set("b64encode", Function::new(ctx.clone(), move |data: String| -> String {
        general_purpose::STANDARD.encode(data)
    }))?;

    encoding.set("b64decode", Function::new(ctx.clone(), move |data: String| -> Result<String> {
        let decoded = general_purpose::STANDARD.decode(data)
            .map_err(|_e| rquickjs::Error::new_from_js("Base64 decode failed", "ValueError"))?;
        String::from_utf8(decoded)
            .map_err(|_e| rquickjs::Error::new_from_js("Invalid UTF-8 after decode", "ValueError"))
    }))?;

    ctx.globals().set("encoding", encoding)?;
    Ok(())
}
