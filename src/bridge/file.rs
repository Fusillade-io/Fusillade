use rquickjs::{Ctx, Function, Result};
use std::fs;

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    ctx.globals().set("open", Function::new(ctx.clone(), move |path: String| -> Result<String> {
        // Security: Prevent reading outside of allowed paths?
        // For now, allow reading relative to CWD.
        // TODO: Enforce security if needed.
        
        match fs::read_to_string(&path) {
            Ok(content) => Ok(content),
            Err(_) => Err(rquickjs::Error::new_from_js("IOError", "String")),
        }
    }))?;
    Ok(())
}
