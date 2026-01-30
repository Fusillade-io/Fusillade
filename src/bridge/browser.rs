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

    #[qjs(rename = "setContent")]
    pub fn set_content(&self, ctx: Ctx<'_>, html: String) -> Result<()> {
        let start = Instant::now();
        let escaped = html.replace('\\', "\\\\").replace('`', "\\`");
        let script = format!(
            "(() => {{ document.open(); document.write(`{}`); document.close(); }})()",
            escaped
        );
        self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("setContent failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::setContent", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::setContent", start, None));
        }
        Ok(())
    }

    pub fn focus(&self, ctx: Ctx<'_>, selector: String) -> Result<()> {
        let start = Instant::now();
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            "(() => {{ const el = document.querySelector('{}'); if (el) el.focus(); else throw new Error('Element not found'); }})()",
            escaped
        );
        self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("Focus failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::focus", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::focus", start, None));
        }
        Ok(())
    }

    pub fn select(&self, ctx: Ctx<'js>, selector: String, values: Vec<String>) -> Result<()> {
        let start = Instant::now();
        let escaped_sel = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let values_js = values
            .iter()
            .map(|v| format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(",");
        let script = format!(
            r#"(() => {{
                const el = document.querySelector('{}');
                if (!el) throw new Error('Element not found');
                const vals = [{}];
                Array.from(el.options).forEach(o => {{ o.selected = vals.includes(o.value); }});
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
            }})()"#,
            escaped_sel, values_js
        );
        self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("Select failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::select", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::select", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "getCookies")]
    pub fn get_cookies(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let start = Instant::now();
        let script = r#"document.cookie.split('; ').filter(Boolean).map(c => {
            const [name, ...rest] = c.split('=');
            return { name, value: rest.join('=') };
        })"#;
        let result = self.inner.evaluate(script, false).map_err(|e| {
            let msg = format!("getCookies failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::getCookies", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::getCookies", start, None));
        }
        let json_str = serde_json::to_string(&result.value).unwrap_or("[]".to_string());
        let json_obj: Object = ctx.globals().get("JSON")?;
        let parse: Function = json_obj.get("parse")?;
        parse.call((json_str,))
    }

    #[qjs(rename = "setCookie")]
    pub fn set_cookie(&self, ctx: Ctx<'_>, cookie: Object<'_>) -> Result<()> {
        let start = Instant::now();
        let name: String = cookie.get("name")?;
        let value: String = cookie.get("value")?;
        let mut cookie_str = format!("{}={}", name, value);
        if let Ok(domain) = cookie.get::<_, String>("domain") {
            cookie_str.push_str(&format!("; domain={}", domain));
        }
        if let Ok(path) = cookie.get::<_, String>("path") {
            cookie_str.push_str(&format!("; path={}", path));
        }
        if let Ok(true) = cookie.get::<_, bool>("secure") {
            cookie_str.push_str("; secure");
        }
        if let Ok(max_age) = cookie.get::<_, i64>("maxAge") {
            cookie_str.push_str(&format!("; max-age={}", max_age));
        }
        let escaped = cookie_str.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!("document.cookie = '{}'", escaped);
        self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("setCookie failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::setCookie", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::setCookie", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "deleteCookie")]
    pub fn delete_cookie(&self, ctx: Ctx<'_>, name: String) -> Result<()> {
        let start = Instant::now();
        let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            "document.cookie = '{}=; expires=Thu, 01 Jan 1970 00:00:01 GMT; path=/'",
            escaped
        );
        self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("deleteCookie failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric(
                    "browser::deleteCookie",
                    start,
                    Some(msg.clone()),
                ));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::deleteCookie", start, None));
        }
        Ok(())
    }

    #[qjs(rename = "queryAll")]
    pub fn query_all(&self, ctx: Ctx<'js>, selector: String) -> Result<Value<'js>> {
        let start = Instant::now();
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            "Array.from(document.querySelectorAll('{}')).map(el => ({{ tag: el.tagName.toLowerCase(), text: el.textContent || '', id: el.id || '', className: el.className || '' }}))",
            escaped
        );
        let result = self.inner.evaluate(&script, false).map_err(|e| {
            let msg = format!("queryAll failed: {}", e);
            if let Some(ref tx) = self.tx {
                let _ = tx.send(make_metric("browser::queryAll", start, Some(msg.clone())));
            }
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        if let Some(ref tx) = self.tx {
            let _ = tx.send(make_metric("browser::queryAll", start, None));
        }
        let json_str = serde_json::to_string(&result.value).unwrap_or("[]".to_string());
        let json_obj: Object = ctx.globals().get("JSON")?;
        let parse: Function = json_obj.get("parse")?;
        parse.call((json_str,))
    }

    #[qjs(rename = "waitForResponse")]
    pub fn wait_for_response(
        &self,
        ctx: Ctx<'js>,
        url_pattern: String,
        timeout_ms: Option<f64>,
    ) -> Result<Value<'js>> {
        let start = Instant::now();
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(30000.0) as u64);
        let escaped_pattern = url_pattern.replace('\\', "\\\\").replace('\'', "\\'");

        loop {
            if start.elapsed() > timeout {
                let msg = format!(
                    "waitForResponse timed out after {}ms waiting for '{}'",
                    timeout.as_millis(),
                    url_pattern
                );
                if let Some(ref tx) = self.tx {
                    let _ = tx.send(make_metric(
                        "browser::waitForResponse",
                        start,
                        Some(msg.clone()),
                    ));
                }
                let _ = ctx.throw(msg.into_js(&ctx).unwrap());
                return Err(rquickjs::Error::Exception);
            }

            let script = format!(
                "(() => {{ const entries = performance.getEntriesByType('resource').filter(e => e.name.includes('{}')); if (entries.length > 0) {{ const e = entries[entries.length - 1]; return {{ name: e.name, duration: e.duration, startTime: e.startTime, transferSize: e.transferSize || 0 }}; }} return null; }})()",
                escaped_pattern
            );

            match self.inner.evaluate(&script, false) {
                Ok(result) => {
                    let is_valid = match &result.value {
                        Some(val) => !val.is_null(),
                        None => false,
                    };
                    if is_valid {
                        if let Some(ref tx) = self.tx {
                            let _ = tx.send(make_metric("browser::waitForResponse", start, None));
                        }
                        let json_str =
                            serde_json::to_string(&result.value).unwrap_or("null".to_string());
                        let json_obj: Object = ctx.globals().get("JSON")?;
                        let parse: Function = json_obj.get("parse")?;
                        return parse.call((json_str,));
                    }
                }
                Err(_) => {
                    // Script evaluation error, continue polling
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        }
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

    #[test]
    fn test_browser_make_metric_set_content() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::setContent", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::setContent");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_focus() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::focus", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::focus");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_select() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::select", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::select");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_get_cookies() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::getCookies", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::getCookies");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_set_cookie() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::setCookie", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::setCookie");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_delete_cookie() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::deleteCookie", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::deleteCookie");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_query_all() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::queryAll", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::queryAll");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }

    #[test]
    fn test_browser_make_metric_wait_for_response() {
        let start = std::time::Instant::now();
        let metric = make_metric("browser::waitForResponse", start, None);
        match metric {
            crate::stats::Metric::Request { name, status, .. } => {
                assert_eq!(name, "browser::waitForResponse");
                assert_eq!(status, 200);
            }
            _ => panic!("Expected Request metric"),
        }
    }
}
