use headless_chrome::{Browser, LaunchOptions, Tab};
use rquickjs::{
    class::{Trace, Tracer},
    Class, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value,
};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct CurrentPage(pub Arc<Mutex<Option<Arc<Tab>>>>);

unsafe impl<'js> JsLifetime<'js> for CurrentPage {
    type Changed<'to> = CurrentPage;
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

        let js_page = JsPage { inner: tab };
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
    }

    pub fn content(&self, ctx: Ctx<'_>) -> Result<String> {
        self.inner.get_content().map_err(|e| {
            let msg = format!("Failed to get content: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })
    }

    pub fn click(&self, ctx: Ctx<'_>, selector: String) -> Result<()> {
        let el = self.inner.find_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        el.click().map_err(|e| {
            let msg = format!("Click failed: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
        Ok(())
    }

    #[qjs(rename = "type")]
    pub fn type_into(&self, ctx: Ctx<'_>, selector: String, text: String) -> Result<()> {
        let el = self.inner.find_element(&selector).map_err(|e| {
            let msg = format!("Element not found: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;

        el.type_into(&text).map_err(|e| {
            let msg = format!("Type failed: {}", e);
            let _ = ctx.throw(msg.into_js(&ctx).unwrap());
            rquickjs::Error::Exception
        })?;
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

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    let current_page = CurrentPage(Arc::new(Mutex::new(None)));
    ctx.store_userdata(current_page)?;

    Class::<JsBrowser>::define(&ctx.globals())?;
    Class::<JsPage>::define(&ctx.globals())?;

    let browser_mod = Object::new(ctx.clone())?;
    browser_mod.set("launch", Function::new(ctx.clone(), launch_browser))?;

    ctx.globals().set("chromium", browser_mod)?;
    Ok(())
}
