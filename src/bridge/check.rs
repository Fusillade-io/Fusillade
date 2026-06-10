use crate::bridge::browser::CurrentPage;
use crate::stats::Metric;
use crossbeam_channel::Sender;
use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
use rquickjs::{Ctx, Function, Object, Result, Value};
use std::fs;

fn capture_screenshot(ctx: &Ctx<'_>, check_name: &str, failure_type: &str) {
    // Only browser tests have a current page; plain HTTP tests have nothing to
    // screenshot, so bail out before touching the filesystem.
    let Some(cp) = ctx.userdata::<CurrentPage>() else {
        return;
    };
    let Ok(guard) = cp.0.lock() else {
        return;
    };
    let Some(tab) = &*guard else {
        return;
    };

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `script` in a fresh JS context with check()/assertion() registered
    /// and returns every Metric::Check sent to the channel, in order.
    fn run_checks(script: &str) -> Vec<(String, bool, Option<String>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();

        context.with(|ctx| {
            register_sync(&ctx, tx).unwrap();
            ctx.eval::<(), _>(script).unwrap();
        });

        rx.try_iter()
            .map(|m| match m {
                Metric::Check {
                    name,
                    success,
                    message,
                } => (name, success, message),
                other => panic!("Expected Metric::Check, got {:?}", other),
            })
            .collect()
    }

    #[test]
    fn test_check_passing_assertion_reports_success() {
        let metrics =
            run_checks(r#"check({status: 200}, {'status is 200': (r) => r.status === 200});"#);
        assert_eq!(metrics, vec![("status is 200".to_string(), true, None)]);
    }

    #[test]
    fn test_check_failing_assertion_reports_failure() {
        let metrics =
            run_checks(r#"check({status: 500}, {'status is 200': (r) => r.status === 200});"#);
        assert_eq!(metrics, vec![("status is 200".to_string(), false, None)]);
    }

    #[test]
    fn test_check_string_return_fails_with_custom_message() {
        let metrics = run_checks(r#"check(null, {'has body': () => 'body was empty'});"#);
        assert_eq!(
            metrics,
            vec![(
                "has body".to_string(),
                false,
                Some("body was empty".to_string())
            )]
        );
    }

    #[test]
    fn test_check_throwing_assertion_fails_with_js_error() {
        let metrics = run_checks(r#"check(null, {'explodes': (r) => r.status === 200});"#);
        assert_eq!(metrics.len(), 1);
        let (name, success, message) = &metrics[0];
        assert_eq!(name, "explodes");
        assert!(!success);
        assert!(
            message
                .as_deref()
                .unwrap_or_default()
                .starts_with("JS error:"),
            "expected JS error message, got {:?}",
            message
        );
    }

    #[test]
    fn test_check_multiple_assertions_report_individually() {
        let metrics = run_checks(
            r#"check({status: 200, body: ''}, {
                'status ok': (r) => r.status === 200,
                'has body': (r) => r.body.length > 0,
            });"#,
        );
        assert_eq!(metrics.len(), 2);
        let by_name: std::collections::HashMap<_, _> = metrics
            .iter()
            .map(|(name, success, _)| (name.as_str(), *success))
            .collect();
        assert_eq!(by_name["status ok"], true);
        assert_eq!(by_name["has body"], false);
    }

    #[test]
    fn test_check_null_and_undefined_results_fail() {
        let metrics = run_checks(
            r#"check(1, {'null result': () => null, 'undefined result': () => undefined});"#,
        );
        assert_eq!(metrics.len(), 2);
        assert!(metrics.iter().all(|(_, success, _)| !success));
    }

    #[test]
    fn test_check_non_bool_non_null_results_pass() {
        // Documented behavior: any result other than false/string/null/undefined
        // counts as a pass — including 0, which diverges from JS truthiness.
        let metrics =
            run_checks(r#"check(1, {'object result': () => ({}), 'zero result': () => 0});"#);
        assert_eq!(metrics.len(), 2);
        assert!(metrics.iter().all(|(_, success, _)| *success));
    }

    #[test]
    fn test_assertion_alias_is_registered() {
        let metrics = run_checks(r#"assertion(2, {'is two': (v) => v === 2});"#);
        assert_eq!(metrics, vec![("is two".to_string(), true, None)]);
    }
}
