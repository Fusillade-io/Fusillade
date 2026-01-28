use crate::bridge::browser::CurrentPage;
use crate::stats::Metric;
use crossbeam_channel::Sender;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use rquickjs::{Ctx, Function, Object, Result, Value};
use std::fs;

fn capture_screenshot(ctx: &Ctx<'_>, check_name: &str, failure_type: &str) {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let filename_base = format!(
        "failure_{}_{}_{}",
        check_name.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '-', "_"),
        failure_type,
        timestamp
    );

    // Ensure directory exists
    if let Err(e) = fs::create_dir_all("screenshots") {
        eprintln!("Failed to create screenshots directory: {}", e);
        return;
    }

    let filename = format!("screenshots/{}.png", filename_base);

    if let Some(cp) = ctx.userdata::<CurrentPage>() {
        if let Ok(guard) = cp.0.lock() {
            if let Some(tab) = &*guard {
                match tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true) {
                    Ok(data) => {
                        if let Err(e) = std::fs::write(&filename, data) {
                            eprintln!("Failed to save screenshot {} : {}", filename, e);
                        } else {
                            println!("Screenshot saved to {}", filename);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error taking screenshot: {}", e);
                    }
                }
            }
        }
    } else {
        eprintln!("CurrentPage userdata not found, cannot take screenshot.");
    }
}

pub fn register_sync<'js>(ctx: &Ctx<'js>, tx: Sender<Metric>) -> Result<()> {
    let check_func = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, val: Value<'js>, checks: Object<'js>| -> Result<()> {
            let tx = tx.clone();
            for key in checks.keys::<String>() {
                let key = key?;
                let func: Function = checks.get(&key)?;

                // Call the assertion function and get the result as a Value
                // This allows us to handle:
                // - true -> pass
                // - false -> fail (no custom message)
                // - string -> fail with custom message
                let (success, message) = match func.call::<_, Value>((val.clone(),)) {
                    Ok(result) => {
                        if result.is_bool() {
                            // Boolean result: true = pass, false = fail
                            (result.as_bool().unwrap_or(false), None)
                        } else if result.is_string() {
                            // String result: fail with custom message
                            let msg = result.as_string().and_then(|s| s.to_string().ok());
                            (false, msg)
                        } else {
                            // Truthy check for other types (numbers, objects, etc.)
                            // null/undefined = false, everything else = true
                            let is_truthy = !result.is_null() && !result.is_undefined();
                            (is_truthy, None)
                        }
                    }
                    Err(e) => {
                        eprintln!("Assertion function '{}' failed: {}", key, e);
                        capture_screenshot(&ctx, &key, "js_error");
                        (false, Some(format!("JS error: {}", e)))
                    }
                };

                let _ = tx.send(Metric::Check {
                    name: key.clone(),
                    success,
                    message: message.clone(),
                });

                if !success {
                    capture_screenshot(&ctx, &key, "assertion_failed");
                }
            }
            Ok(())
        },
    )?;

    ctx.globals().set("assertion", check_func.clone())?;
    ctx.globals().set("check", check_func)?;

    Ok(())
}
