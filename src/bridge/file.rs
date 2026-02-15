use rquickjs::{Ctx, Function, Result};
use std::fs;
use std::path::Path;

/// Validate that a file path is safe: no absolute paths, no `..` traversal.
fn validate_path(path: &str) -> std::result::Result<(), String> {
    let p = Path::new(path);

    if p.is_absolute() {
        return Err(format!(
            "Absolute paths are not allowed: '{}'. Use a path relative to your script directory.",
            path
        ));
    }

    for component in p.components() {
        if let std::path::Component::ParentDir = component {
            return Err(format!(
                "Path traversal ('..') is not allowed: '{}'. Use a path relative to your script directory.",
                path
            ));
        }
    }

    Ok(())
}

fn read_file(path: &str) -> std::result::Result<String, std::io::Error> {
    validate_path(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::PermissionDenied, e))?;
    fs::read_to_string(path)
}

pub fn register_sync(ctx: &Ctx) -> Result<()> {
    ctx.globals().set(
        "open",
        Function::new(ctx.clone(), move |path: String| -> Result<String> {
            read_file(&path).map_err(|_| rquickjs::Error::new_from_js("IOError", "String"))
        }),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn relative_name(file: &NamedTempFile) -> String {
        file.path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn test_read_existing_file() {
        let mut file = NamedTempFile::new_in(".").unwrap();
        writeln!(file, "hello world").unwrap();

        let content = read_file(&relative_name(&file)).unwrap();
        assert_eq!(content.trim(), "hello world");
    }

    #[test]
    fn test_read_nonexistent_file() {
        let result = read_file("nonexistent_file_that_does_not_exist.txt");
        assert!(result.is_err());
    }

    #[test]
    fn test_read_empty_file() {
        let file = NamedTempFile::new_in(".").unwrap();
        let content = read_file(&relative_name(&file)).unwrap();
        assert_eq!(content, "");
    }

    #[test]
    fn test_read_unicode_content() {
        let mut file = NamedTempFile::new_in(".").unwrap();
        writeln!(file, "héllo wörld 日本語").unwrap();

        let content = read_file(&relative_name(&file)).unwrap();
        assert!(content.contains("héllo"));
        assert!(content.contains("日本語"));
    }

    #[test]
    fn test_read_multiline_file() {
        let mut file = NamedTempFile::new_in(".").unwrap();
        writeln!(file, "line1").unwrap();
        writeln!(file, "line2").unwrap();
        writeln!(file, "line3").unwrap();

        let content = read_file(&relative_name(&file)).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_rejects_absolute_path() {
        let result = read_file("/etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Absolute paths are not allowed"));
    }

    #[test]
    fn test_rejects_dot_dot_traversal() {
        let result = read_file("../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("Path traversal"));
    }

    #[test]
    fn test_rejects_hidden_traversal() {
        let result = read_file("data/../../../etc/passwd");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Path traversal"));
    }

    #[test]
    fn test_allows_relative_path() {
        let mut file = NamedTempFile::new_in(".").unwrap();
        writeln!(file, "test data").unwrap();

        let filename = file.path().file_name().unwrap().to_str().unwrap();
        let content = read_file(filename).unwrap();
        assert_eq!(content.trim(), "test data");
    }

    #[test]
    fn test_allows_subdirectory_path() {
        assert!(validate_path("data/users.csv").is_ok());
        assert!(validate_path("nested/dir/file.json").is_ok());
    }

    #[test]
    fn test_validate_path_edge_cases() {
        assert!(validate_path("simple.txt").is_ok());
        assert!(validate_path("./data.csv").is_ok());
        assert!(validate_path("../secret").is_err());
        assert!(validate_path("/root/file").is_err());
    }
}
