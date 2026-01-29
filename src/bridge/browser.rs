use crate::stats::{Metric, RequestTimings};
use crossbeam_channel::Sender;
use headless_chrome::{Browser, LaunchOptions, Tab};
use rquickjs::{
    class::{Trace, Tracer},
    Class, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn make_metric(name: &str, start: Instant, error: Option<String>) -> Metric {
    let duration = start.elapsed();
    let timings = RequestTimings {
        duration,
        waiting: duration,
        ..Default::default()
    };
    let status = if error.is_none() { 200 } else { 0 };
    Metric::Request {
        name: name.to_string(),
        timings,
        status,
        error,
        tags: HashMap::new(),
    }
}

#[derive(Clone)]
pub struct CurrentPage(pub Arc<Mutex<Option<Arc<Tab>>>>);

unsafe impl<'js> JsLifetime<'js> for CurrentPage {
    type Changed<'to> = CurrentPage;
}

#[derive(Clone)]
struct BrowserMetricSender(Sender<Metric>);
unsafe impl<'js> JsLifetime<'js> for BrowserMetricSender {
    type Changed<'to> = BrowserMetricSender;
}

#[rquickjs::class]
pub struct JsBrowser {
    inner: Arc<Browser>,
}

impl<'js> Trace<'js> for JsBrowser {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for JsBrowser {
    type Changed<'to> = JsBrowser;
}

#[rquickjs::methods]
impl JsBrowser {
    #[qjs(rename = "newPage")]
    pub fn new_page<'js>(&self, ctx: Ctx<'js>) -> Result<Class<'js, JsPage>> {
        let tab = self.inner.new_tab().map_err(|e| {
            let msg = format!("Failed to create tab: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(cp) = ctx.userdata::<CurrentPage>() {
            let mut guard = cp.0.lock().unwrap();
            *guard = Some(tab.clone());
        }

        let tx = ctx.userdata::<BrowserMetricSender>().map(|w| w.0.clone());
        let js_page = JsPage { inner: tab, tx };
        Class::<JsPage>::instance(ctx, js_page)
    }

    pub fn close(&self, ctx: Ctx<'_>) -> Result<()> {
        if let Some(cp) = ctx.userdata::<CurrentPage>() {
            let mut guard = cp.0.lock().unwrap();
            *guard = None;
        }
        Ok(())
    }
}

#[rquickjs::class]
pub struct JsPage {
    inner: Arc<Tab>,
    tx: Option<Sender<Metric>>,
}

impl<'js> Trace<'js> for JsPage {
    fn trace<'a>(&self, _tracer: Tracer<'a, 'js>) {}
}

unsafe impl<'js> JsLifetime<'js> for JsPage {
    type Changed<'to> = JsPage;
}

#[rquickjs::methods]
impl<'js> JsPage {
    pub fn goto(&self, ctx: Ctx<'_>, url: String) -> Result<()> {
        let start = Instant::now();
        let result = (|| -> Result<()> {
            self.inner.navigate_to(&url).map_err(|e| {
                let msg = format!("Navigation failed: {}", e);
                let _ = ctx.throw(msg.into_js(&ctx).unwrap());
                rquickjs::Error::Exception
            })?;
            self.inner.wait_until_navigated().map_err(|e| {
                let msg = format!("Wait failed: {}", e);
                let _ = ctx.throw(msg.into_js(&ctx).unwrap());
                rquickjs::Error::Exception
            })?;
            Ok(())
        })();
        match &result {
            Ok(()) => {
                if let Some(ref tx) = self.tx {
                    let _ = tx.send(make_metric("browser::goto", start, None));
                }
            }
            Err(_) => {
                if let Some(ref tx) = self.tx {
                    let _ = tx.send(make_metric(
                        "browser::goto",
                        start,
                        Some("Navigation failed".to_string()),
                    ));
                }
            }
        }
        result
    }

    pub fn content(&self, ctx: Ctx<'_>) -> Result<String> {
        self.inner.get_content().map_err(|e| {
            let msg = format!("Failed to get content: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })
    }

    pub fn click(&self, ctx: Ctx<'_>, selector: String) -> Result<()> {
        let start = Instant::now();
        let el = self.inner.find_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::click", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        el.click().map_err(|e| {
            let msg = format!("Click failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::click", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::click", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "type")]
    pub fn type_into(&self, ctx: Ctx<'_>, selector: String, text: String) -> Result<()> {
        let start = Instant::now();
        let el = self.inner.find_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::type", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        el.type_into(&text).map_err(|e| {
            let msg = format!("Type failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::type", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::type", start, None));
        }
        Ok(())
    }

    pub fn evaluate(&self, ctx: Ctx<'js>, script: String) -> Result<Value<'js>> {
        let result = self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("Evaluation failed: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        let json_str = serde_json::to_string(&result.value).unwrap_or("null".to_string());
        let json_obj: Object = ctx.globals().get("JSON")?;
        let parse: Function = json_obj.get("parse")?;
        parse.call((json_str,))
    }

    pub fn metrics(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        // performance.timing properties are not own properties, so we construct an object manually
        // Return object literal directly so evaluate handles serialization/deserialization transparently
        let script = r#"
            ({
                navigationStart: window.performance.timing.navigationStart,
                domInteractive: window.performance.timing.domInteractive,
                domComplete: window.performance.timing.domComplete,
                loadEventEnd: window.performance.timing.loadEventEnd
            })
        "#;
        self.evaluate(ctx, script.to_string())
    }

    pub fn screenshot(&self, ctx: Ctx<'_>) -> Result<Vec<u8>> {
        use headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption;
        self.inner
            .capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true)
            .map_err(|e| {
                let msg = format!("Screenshot failed: {}", e);
                let _ = ctx.throw(msg.into_js(&ctx).unwrap());
                rquickjs::Error::Exception
            })
    }

    pub fn url(&self) -> String {
        self.inner.get_url()
    }

    pub fn title(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.evaluate(ctx, "document.title".to_string())
    }

    pub fn fill(&self, ctx: Ctx<'_>, selector: String, text: String) -> Result<()> {
        let start = Instant::now();

        // Clear the input value via JavaScript
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let clear_script = format!(
            "(() => {{ const el = document.querySelector('{}'); if (el) {{ el.value = ''; el.dispatchEvent(new Event('input', {{bubbles: true}})); }} }})()",
            escaped
        );
        self.inner.evaluate(&clear_script, false).map_err(|e| {
            let msg = format!("Failed to clear element: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::fill", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        // Type the new text
        let el = self.inner.find_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::fill", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        el.type_into(&text).map_err(|e| {
            let msg = format!("Fill failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::fill", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::fill", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "waitForSelector")]
    pub fn wait_for_selector(&self, ctx: Ctx<'_>, selector: String) -> Result<()> {
        let start = Instant::now();

        self.inner.wait_for_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric(
                    "browser::waitForSelector",
                    start,
                    Some(msg.clone()),
                ));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::waitForSelector", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "waitForNavigation")]
    pub fn wait_for_navigation(&self, ctx: Ctx<'_>) -> Result<()> {
        let start = Instant::now();

        self.inner.wait_until_navigated().map_err(|e| {
            let msg = format!("Navigation wait failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric(
                    "browser::waitForNavigation",
                    start,
                    Some(msg.clone()),
                ));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::waitForNavigation", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "waitForTimeout")]
    pub fn wait_for_timeout(&self, ms: f64) -> Result<()> {
        std::thread::sleep(Duration::from_millis(ms as u64));
        Ok(())
    }

    pub fn close(&self, ctx: Ctx<'_>) -> Result<()> {
        // Clear the current page reference if this page is the active one
        if let Some(cp) = ctx.userdata::<CurrentPage>() {
            let mut guard = cp.0.lock().unwrap();
            if let Some(ref current) = *guard {
                if Arc::ptr_eq(current, &self.inner) {
                    *guard = None;
                }
            }
        }
        let _ = self.inner.close_target();
        Ok(())
    }
}

fn launch_browser<'js>(ctx: Ctx<'js>) -> Result<Class<'js, JsBrowser>> {
    let options = LaunchOptions::default();
    let browser = Browser::new(options).map_err(|e| {
        let msg = format!("Failed to launch browser: {}", e);
        let _ = ctx.throw(msg.into_js(&ctx).unwrap());
        rquickjs::Error::Exception
    })?;

    let js_browser = JsBrowser {
        inner: Arc::new(browser),
    };
    Class::<JsBrowser>::instance(ctx, js_browser)
}

pub fn register_sync(ctx: &Ctx, tx: Sender<Metric>) -> Result<()> {
    let current_page = CurrentPage(Arc::new(Mutex::new(None)));
    ctx.store_userdata(current_page)?;
    ctx.store_userdata(BrowserMetricSender(tx))?;

    Class::<JsBrowser>::define(&ctx.globals())?;
    Class::<JsPage>::define(&ctx.globals())?;

    let browser_mod = Object::new(ctx.clone())?;
    browser_mod.set("launch", Function::new(ctx.clone(), launch_browser))?;

    ctx.globals().set("chromium", browser_mod)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_browser_make_metric_success() {
        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let metric = make_metric("browser::goto", start, None);

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "browser::goto");
                assert_eq!(status, 200);
                assert!(error.is_none());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_error() {
        let start = std::time::Instant::now();
        let metric = make_metric(
            "browser::click",
            start,
            Some("Element not found".to_string()),
        );

        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "browser::click");
                assert_eq!(status, 0);
                assert!(error.is_some());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_duration_recorded() {
        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let metric = make_metric("browser::goto", start, None);

        match metric {
            crate::stats::Metric::Request { timings, .. } => {
                assert!(timings.duration.as_millis() >= 10);
                assert_eq!(timings.duration, timings.waiting);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_wait_for_selector() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::waitForSelector", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::waitForSelector");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_wait_for_selector_error() {
        let start = std::time::Instant::now();
        let metric = make_metric(
            "browser::waitForSelector",
            start,
            Some("Element not found: #missing".to_string()),
        );
        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "browser::waitForSelector");
                assert_eq!(status, 0);
                assert_eq!(error.unwrap(), "Element not found: #missing");
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_fill() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::fill", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::fill");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_fill_error() {
        let start = std::time::Instant::now();
        let metric = make_metric(
            "browser::fill",
            start,
            Some("Failed to clear element: timeout".to_string()),
        );
        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "browser::fill");
                assert_eq!(status, 0);
                assert!(error.unwrap().contains("Failed to clear element"));
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_wait_for_navigation() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::waitForNavigation", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::waitForNavigation");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_wait_for_navigation_error() {
        let start = std::time::Instant::now();
        let metric = make_metric(
            "browser::waitForNavigation",
            start,
            Some("Navigation wait failed".to_string()),
        );
        match metric {
            crate::stats::Metric::Request {
                name,
                status,
                error,
                ..
            } => {
                assert_eq!(name, "browser::waitForNavigation");
                assert_eq!(status, 0);
                assert!(error.is_some());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_tags_empty() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::goto", start, None);
        match metric {
            crate::stats::Metric::Request { tags, .. } => {
                assert!(tags.is_empty());
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_all_operations() {
        let operations = vec![
            "browser::goto",
            "browser::click",
            "browser::type",
            "browser::fill",
            "browser::waitForSelector",
            "browser::waitForNavigation",
        ];
        for op in operations {
            let start = std::time::Instant::now();
            let success = make_metric(op, start, None);
            let failure = make_metric(op, start, Some("error".to_string()));
            match success {
                crate::stats::Metric::Request { name, status, .. } => {
                    assert_eq!(name, op);
                    assert_eq!(status, 200);
                }
                _ => panic!("Expected Request metric for {}", op),
            }
            match failure {
                crate::stats::Metric::Request { name, status, .. } => {
                    assert_eq!(name, op);
                    assert_eq!(status, 0);
                }
                _ => panic!("Expected Request metric for {}", op),
            }
        }
    }
}
