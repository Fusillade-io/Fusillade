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

fn set_group(name: &str) -> String {
    CURRENT_GROUP.with(|g: &RefCell<String>| {
        let mut g = g.borrow_mut();
        let prev = g.clone();
        if g.is_empty() {
            *g = name.to_string();
        } else {
            *g = format!("{}::{}", g, name);
        }
        prev
    })
}

fn restore_group(prev: String) {
    CURRENT_GROUP.with(|g: &RefCell<String>| {
        *g.borrow_mut() = prev;
    });
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    ctx.globals().set("segment", Function::new(ctx.clone(), move |name: String, func: Function| -> Result<()> {
        let prev = set_group(&name);
        let res: Result<()> = func.call(());
        restore_group(prev);
        res
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_group() {
        CURRENT_GROUP.with(|g| {
            *g.borrow_mut() = String::new();
        });
    }

    #[test]
    fn test_empty_group_prefix() {
        reset_group();
        assert_eq!(get_current_group_prefix(), "");
    }

    #[test]
    fn test_single_group() {
        reset_group();
        let prev = set_group("login");
        assert_eq!(get_current_group_prefix(), "login::");
        restore_group(prev);
        assert_eq!(get_current_group_prefix(), "");
    }

    #[test]
    fn test_nested_groups() {
        reset_group();
        let prev1 = set_group("auth");
        assert_eq!(get_current_group_prefix(), "auth::");

        let prev2 = set_group("login");
        assert_eq!(get_current_group_prefix(), "auth::login::");

        let prev3 = set_group("validation");
        assert_eq!(get_current_group_prefix(), "auth::login::validation::");

        restore_group(prev3);
        assert_eq!(get_current_group_prefix(), "auth::login::");

        restore_group(prev2);
        assert_eq!(get_current_group_prefix(), "auth::");

        restore_group(prev1);
        assert_eq!(get_current_group_prefix(), "");
    }

    #[test]
    fn test_group_restore_on_unwind() {
        reset_group();
        let prev = set_group("test");
        assert_eq!(get_current_group_prefix(), "test::");

        // Simulate early return/error by just restoring
        restore_group(prev);
        assert_eq!(get_current_group_prefix(), "");
    }

    #[test]
    fn test_special_chars_in_group_name() {
        reset_group();
        let prev = set_group("user-login_v2");
        assert_eq!(get_current_group_prefix(), "user-login_v2::");
        restore_group(prev);
    }
}
