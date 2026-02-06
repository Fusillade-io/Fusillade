use anyhow::Result;
use std::fs;
use std::path::Path;

const DEFAULT_SCRIPT: &str = r#"// Fusillade Load Test Script
// Documentation: https://fusillade.io/docs

export const options = {
    workers: 10,
    duration: '30s',
    thresholds: {
        'http_req_duration': ['p95 < 500'],
        'http_req_failed': ['rate < 0.01'],
    },
};

export default function() {
    const res = http.get('https://httpbin.org/get');

    check(res, {
        'status is 200': (r) => r.status === 200,
    });

    sleep(1);
}
"#;

const DEFAULT_CONFIG: &str = r#"# Fusillade Configuration
# Use with: fusillade run script.js --config fusillade.yaml

workers: 10
duration: 30s

# Ramping schedule (optional, overrides workers/duration)
# schedule:
#   - duration: 10s
#     target: 5
#   - duration: 20s
#     target: 10
#   - duration: 10s
#     target: 0

# Pass/fail criteria
criteria:
  http_req_duration:
    - p95<500
  http_req_failed:
    - rate<0.01
"#;

/// Initialize a new Fusillade project with starter files.
pub fn run_init(output: Option<&Path>, with_config: bool) -> Result<()> {
    let script_path = output.unwrap_or(Path::new("test.js"));

    if script_path.exists() {
        anyhow::bail!(
            "File already exists: {:?}. Remove it first or choose a different output path.",
            script_path
        );
    }

    // Create parent directories if needed
    if let Some(parent) = script_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(script_path, DEFAULT_SCRIPT)?;
    println!("✓ Created {}", script_path.display());

    if with_config {
        let config_path = Path::new("fusillade.yaml");
        if !config_path.exists() {
            fs::write(config_path, DEFAULT_CONFIG)?;
            println!("✓ Created fusillade.yaml");
        }
    }

    println!("\nRun your test with:");
    println!("  fusillade run {}", script_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_run_init_creates_script() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.js");

        run_init(Some(&script_path), false).unwrap();

        assert!(script_path.exists());
        let content = fs::read_to_string(&script_path).unwrap();
        assert!(content.contains("export const options"));
        assert!(content.contains("export default function"));
    }

    #[test]
    fn test_run_init_with_config() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.js");

        // Change to temp dir so fusillade.yaml is created there
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        run_init(Some(&script_path), true).unwrap();

        std::env::set_current_dir(original_dir).unwrap();

        assert!(script_path.exists());
        assert!(temp_dir.path().join("fusillade.yaml").exists());
    }

    #[test]
    fn test_run_init_fails_if_exists() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.js");

        fs::write(&script_path, "existing content").unwrap();

        let result = run_init(Some(&script_path), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_script_has_valid_content() {
        assert!(DEFAULT_SCRIPT.contains("https://httpbin.org/get"));
        assert!(DEFAULT_SCRIPT.contains("check(res"));
        assert!(DEFAULT_SCRIPT.contains("sleep(1)"));
    }

    #[test]
    fn test_default_config_has_valid_content() {
        assert!(DEFAULT_CONFIG.contains("workers:"));
        assert!(DEFAULT_CONFIG.contains("duration:"));
        assert!(DEFAULT_CONFIG.contains("criteria:"));
    }
}
