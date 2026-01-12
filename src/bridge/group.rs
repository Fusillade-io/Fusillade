use rquickjs::{Ctx, Function, Result};
use std::cell::RefCell;

thread_local! {
    pub static CURRENT_GROUP: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn get_current_group_prefix() -> String {
    CURRENT_GROUP.with(|g| {
        let g = g.borrow();
        if g.is_empty() {
            "".to_string()
        } else {
            format!("{}::", *g)
        }
    })
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    ctx.globals().set("segment", Function::new(ctx.clone(), move |name: String, func: Function| -> Result<()> {
        let prev = CURRENT_GROUP.with(|g: &RefCell<String>| {
            let mut g = g.borrow_mut();
            let p = g.clone();
            if g.is_empty() {
                *g = name;
            } else {
                *g = format!("{}::{}", g, name);
            }
            p
        });

        let res: Result<()> = func.call(());

        CURRENT_GROUP.with(|g: &RefCell<String>| {
            *g.borrow_mut() = prev;
        });

        res
    }))?;
    Ok(())
}
