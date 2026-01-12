use rquickjs::{Ctx, Function, Result, Value, Object};
use crossbeam_channel::Sender;
use crate::stats::Metric;
use crate::bridge::browser::CurrentPage;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use std::fs;

fn capture_screenshot(ctx: &Ctx<'_>, check_name: &str, failure_type: &str) {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let filename_base = format!("failure_{}_{}_{}", check_name.replace(|c: char| !c.is_alphanumeric() && c != '.' && c != '-' , "_"), failure_type, timestamp);
    
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
    let check_func = Function::new(ctx.clone(), move |ctx: Ctx<'js>, val: Value<'js>, checks: Object<'js>| -> Result<()> {
        let tx = tx.clone();
        for key in checks.keys::<String>() {
            let key = key?;
            let func: Function = checks.get(&key)?;
            
            let success = match func.call::<_, bool>((val.clone(),)) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Assertion function '{}' failed: {}", key, e);
                    capture_screenshot(&ctx, &key, "js_error");
                    false // Treat JS error as a failure
                }
            };

            let _ = tx.send(Metric::Check {
                name: key.clone(),
                success,
            });

            if !success {
                capture_screenshot(&ctx, &key, "assertion_failed");
            }
        }
        Ok(())
    })?;
    
    ctx.globals().set("assertion", check_func.clone())?;
    ctx.globals().set("check", check_func)?;
    
    Ok(())
}
