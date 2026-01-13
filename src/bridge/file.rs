use rquickjs::{Ctx, Function, Result};
use std::fs;

fn read_file(path: &str) -> std::result::Result<String, std::io::Error> {
    fs::read_to_string(path)
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    ctx.globals().set("open", Function::new(ctx.clone(), move |path: String| -> Result<String> {
        // Security: Prevent reading outside of allowed paths?
        // For now, allow reading relative to CWD.
        // TODO: Enforce security if needed.

        read_file(&path)
            .map_err(|_| rquickjs::Error::new_from_js("IOError", "String"))
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_existing_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "hello world").unwrap();

        let content = read_file(file.path().to_str().unwrap()).unwrap();
        assert_eq!(content.trim(), "hello world");
    }

    #[test]
    fn test_read_nonexistent_file() {
        let result = read_file("/nonexistent/path/to/file.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let content = read_file(file.path().to_str().unwrap()).unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn test_read_unicode_content() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "héllo wörld 日本語").unwrap();

        let content = read_file(file.path().to_str().unwrap()).unwrap();
        assert!(content.contains("héllo"));
        assert!(content.contains("日本語"));
    }

    #[test]
    fn test_read_multiline_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "line1").unwrap();
        writeln!(file, "line2").unwrap();
        writeln!(file, "line3").unwrap();

        let content = read_file(file.path().to_str().unwrap()).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
    }
}
