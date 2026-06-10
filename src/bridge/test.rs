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
        // Rust-side Value equality compares raw QuickJS value bits (pointer
        // identity for strings, undefined padding for bools), so delegate to
        // real JS strict equality instead.
        let strict_eq: Function = ctx.eval("(a, b) => a === b")?;
        let equal: bool = strict_eq.call((self.actual.clone(), expected))?;
        if equal {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Evaluates `expr` in a fresh JS context with expect()/test()/describe()
    /// registered. Returns Ok(()) if it ran cleanly, or the thrown error's
    /// string form.
    fn eval_expect(expr: &str) -> std::result::Result<(), String> {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();

        context.with(|ctx| {
            register_sync(&ctx).unwrap();
            match ctx.eval::<(), _>(expr) {
                Ok(()) => Ok(()),
                Err(_) => {
                    let caught = ctx.catch();
                    Err(format!("{:?}", caught))
                }
            }
        })
    }

    #[test]
    fn test_to_be_equal_numbers_pass() {
        assert!(eval_expect("expect(42).toBe(42)").is_ok());
    }

    #[test]
    fn test_to_be_different_numbers_throw() {
        let err = eval_expect("expect(1).toBe(2)").unwrap_err();
        assert!(err.contains("AssertionError"), "got: {}", err);
    }

    #[test]
    fn test_to_be_equal_strings_pass() {
        assert!(eval_expect("expect('abc').toBe('abc')").is_ok());
    }

    #[test]
    fn test_to_be_equal_bools_pass() {
        assert!(eval_expect("expect(true).toBe(true)").is_ok());
    }

    #[test]
    fn test_to_be_compares_strings_by_value_not_identity() {
        // 'ab' + 'c' builds a fresh heap string: === must still see it as
        // equal to the literal 'abc'.
        assert!(eval_expect("expect('ab' + 'c').toBe('abc')").is_ok());
    }

    #[test]
    fn test_to_be_number_vs_string_throws() {
        let err = eval_expect("expect(1).toBe('1')").unwrap_err();
        assert!(err.contains("AssertionError"), "got: {}", err);
    }

    #[test]
    fn test_to_equal_deep_objects_pass() {
        assert!(eval_expect("expect({a: 1, b: [1, 2]}).toEqual({a: 1, b: [1, 2]})").is_ok());
    }

    #[test]
    fn test_to_equal_different_objects_throw() {
        let err = eval_expect("expect({a: 1}).toEqual({a: 2})").unwrap_err();
        assert!(err.contains("AssertionError"), "got: {}", err);
    }

    #[test]
    fn test_to_be_truthy_truthy_values_pass() {
        for expr in [
            "expect(1).toBeTruthy()",
            "expect('x').toBeTruthy()",
            "expect({}).toBeTruthy()",
            "expect([]).toBeTruthy()",
            "expect(true).toBeTruthy()",
        ] {
            assert!(eval_expect(expr).is_ok(), "expected pass: {}", expr);
        }
    }

    #[test]
    fn test_to_be_truthy_falsy_values_throw() {
        for expr in [
            "expect(0).toBeTruthy()",
            "expect('').toBeTruthy()",
            "expect(null).toBeTruthy()",
            "expect(undefined).toBeTruthy()",
            "expect(false).toBeTruthy()",
            "expect(NaN).toBeTruthy()",
        ] {
            let err = eval_expect(expr).expect_err(&format!("expected AssertionError: {}", expr));
            assert!(err.contains("AssertionError"), "got: {}", err);
        }
    }

    #[test]
    fn test_test_swallows_assertion_failures() {
        // test() reports failures to stdout but must not propagate them,
        // so one failed test block doesn't abort the iteration.
        assert!(eval_expect("test('fails', () => { expect(1).toBe(2); })").is_ok());
    }

    #[test]
    fn test_describe_runs_body() {
        assert!(eval_expect(
            "let ran = false; describe('suite', () => { ran = true; }); if (!ran) throw 'body not run';"
        )
        .is_ok());
    }
}
