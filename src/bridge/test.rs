use rquickjs::{
    class::{Trace, Tracer},
    Class, Ctx, Function, IntoJs, JsLifetime, Object, Result, Value,
};

#[rquickjs::class]
#[derive(Clone)]
pub struct JsExpectation<'js> {
    actual: Value<'js>,
}

impl<'js> Trace<'js> for JsExpectation<'js> {
    fn trace<'a>(&self, tracer: Tracer<'a, 'js>) {
        self.actual.trace(tracer);
    }
}

unsafe impl<'js> JsLifetime<'js> for JsExpectation<'js> {
    type Changed<'to> = JsExpectation<'to>;
}

#[rquickjs::methods]
impl<'js> JsExpectation<'js> {
    #[qjs(rename = "toBe")]
    pub fn to_be(&self, ctx: Ctx<'js>, expected: Value<'js>) -> Result<()> {
        if self.actual == expected {
            Ok(())
        } else {
            let msg = "AssertionError: Expected values to be strictly equal";
            let err = msg.into_js(&ctx)?;
            Err(ctx.throw(err))
        }
    }

    #[qjs(rename = "toEqual")]
    pub fn to_equal(&self, ctx: Ctx<'js>, expected: Value<'js>) -> Result<()> {
        let json: Object = ctx.globals().get("JSON")?;
        let stringify: Function = json.get("stringify")?;
        let s1: String = stringify.call((self.actual.clone(),))?;
        let s2: String = stringify.call((expected,))?;

        if s1 == s2 {
            Ok(())
        } else {
            let msg = format!("AssertionError: Expected {} to equal {}", s1, s2);
            let err = msg.into_js(&ctx)?;
            Err(ctx.throw(err))
        }
    }

    #[qjs(rename = "toBeTruthy")]
    pub fn to_be_truthy(&self, ctx: Ctx<'js>) -> Result<()> {
        // Follow JavaScript truthiness rules:
        // false, 0, "", null, undefined, NaN are falsy; everything else is truthy
        let is_truthy = if self.actual.is_null() || self.actual.is_undefined() {
            false
        } else if let Some(b) = self.actual.as_bool() {
            b
        } else if let Some(n) = self.actual.as_int() {
            n != 0
        } else if let Some(n) = self.actual.as_float() {
            n != 0.0 && !n.is_nan()
        } else if let Some(s) = self.actual.as_string() {
            !s.to_string().unwrap_or_default().is_empty()
        } else {
            // Objects, arrays, functions are truthy
            true
        };

        if is_truthy {
            Ok(())
        } else {
            let msg = "AssertionError: Expected value to be truthy";
            let err = msg.into_js(&ctx)?;
            Err(ctx.throw(err))
        }
    }
}

fn expect_impl<'js>(ctx: Ctx<'js>, actual: Value<'js>) -> Result<Class<'js, JsExpectation<'js>>> {
    Class::instance(ctx, JsExpectation { actual })
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    let globals = ctx.globals();

    rquickjs::Class::<JsExpectation>::define(&globals)?;

    globals.set(
        "describe",
        Function::new(
            ctx.clone(),
            move |name: String, func: Function| -> Result<()> {
                println!("describe: {}", name);
                func.call::<_, ()>(())?;
                Ok(())
            },
        ),
    )?;

    globals.set(
        "test",
        Function::new(
            ctx.clone(),
            move |name: String, func: Function| -> Result<()> {
                match func.call::<_, ()>(()) {
                    Ok(_) => {
                        println!("  ✓ {}", name);
                        Ok(())
                    }
                    Err(e) => {
                        println!("  ✗ {}", name);
                        println!("    Error: {}", e);
                        Ok(())
                    }
                }
            },
        ),
    )?;

    globals.set("expect", Function::new(ctx.clone(), expect_impl))?;

    Ok(())
}
