use anyhow::Result;
use std::path::Path;

use crate::engine::Engine;

/// Validate a Fusillade script without running it.
/// Checks for:
/// - JavaScript syntax errors
/// - Configuration validity
/// - Import resolution
pub fn run_validate(scenario: &Path, config_path: Option<&Path>) -> Result<()> {
    println!("Validating {}...", scenario.display());

    // Read script content
    let script_content = std::fs::read_to_string(scenario)
        .map_err(|e| anyhow::anyhow!("Failed to read script: {}", e))?;

    // Create engine to validate
    let engine = Engine::new()?;

    // Extract and validate config
    match engine.extract_config(scenario.to_path_buf(), script_content.clone()) {
        Ok(Some(config)) => {
            println!("  ✓ Script syntax OK");
            println!("  ✓ Configuration parsed");

            // Report key config values
            if let Some(w) = config.workers {
                println!("    workers: {}", w);
            }
            if let Some(d) = &config.duration {
                println!("    duration: {}", d);
            }
            if let Some(schedule) = &config.schedule {
                println!("    stages: {} stage(s)", schedule.len());
            }
            if let Some(scenarios) = &config.scenarios {
                println!("    scenarios: {:?}", scenarios.keys().collect::<Vec<_>>());
            }
        }
        Ok(None) => {
            println!("  ✓ Script syntax OK");
            println!("  ⚠ No 'export const options' found (using defaults)");
        }
        Err(e) => {
            println!("  ✗ Validation failed: {}", e);
            return Err(e);
        }
    }

    // Validate external config if provided
    if let Some(cfg_path) = config_path {
        let cfg_content = std::fs::read_to_string(cfg_path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file: {}", e))?;

        // Try YAML first, then JSON
        let ext = cfg_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let result: Result<crate::cli::config::Config, _> = if ext == "json" {
            serde_json::from_str(&cfg_content).map_err(|e| e.to_string())
        } else {
            serde_yaml::from_str(&cfg_content).map_err(|e| e.to_string())
        };

        match result {
            Ok(_) => println!("  ✓ Config file valid: {}", cfg_path.display()),
            Err(e) => {
                println!("  ✗ Config file invalid: {}", e);
                anyhow::bail!("Config validation failed");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const VALID_SCRIPT: &str = r#"
export const options = {
    workers: 10,
    duration: '30s',
};

export default function() {
    http.get('https://example.com');
}
"#;

    const SCRIPT_NO_OPTIONS: &str = r#"
export default function() {
    http.get('https://example.com');
}
"#;

    #[test]
    fn test_validate_valid_script() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("valid.js");
        fs::write(&script_path, VALID_SCRIPT).unwrap();

        let result = run_validate(&script_path, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_script_without_options() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("no_options.js");
        fs::write(&script_path, SCRIPT_NO_OPTIONS).unwrap();

        // Should still pass - options are optional
        let result = run_validate(&script_path, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_file() {
        let path = Path::new("/nonexistent/file.js");
        let result = run_validate(path, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_with_valid_yaml_config() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.js");
        let config_path = temp_dir.path().join("config.yaml");

        fs::write(&script_path, VALID_SCRIPT).unwrap();
        fs::write(&config_path, "workers: 10\nduration: 30s\n").unwrap();

        let result = run_validate(&script_path, Some(&config_path));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_with_invalid_config() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("test.js");
        let config_path = temp_dir.path().join("config.yaml");

        fs::write(&script_path, VALID_SCRIPT).unwrap();
        fs::write(&config_path, "invalid: [yaml: content").unwrap();

        let result = run_validate(&script_path, Some(&config_path));
        assert!(result.is_err());
    }
}
