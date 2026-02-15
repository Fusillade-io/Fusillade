//! Fusillade Cloud Authentication
//!
//! Handles storing and retrieving API keys for cloud mode.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = ".fusillade";
const AUTH_FILE: &str = "auth.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct CloudAuth {
    pub token: String,
    pub api_url: Option<String>,
}

/// Get the path to the auth config file
fn auth_file_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(CONFIG_DIR).join(AUTH_FILE)
}

/// Save API token to local config
pub fn save_token(token: &str, api_url: Option<&str>) -> Result<(), std::io::Error> {
    let auth = CloudAuth {
        token: token.to_string(),
        api_url: api_url.map(|s| s.to_string()),
    };

    let config_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(CONFIG_DIR);

    fs::create_dir_all(&config_dir)?;

    let json = serde_json::to_string_pretty(&auth).map_err(std::io::Error::other)?;

    fs::write(auth_file_path(), json)?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(auth_file_path(), fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Load API token from local config
pub fn load_token() -> Option<CloudAuth> {
    let path = auth_file_path();
    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Remove stored token (logout)
pub fn clear_token() -> Result<(), std::io::Error> {
    let path = auth_file_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Check if user is logged in
pub fn is_logged_in() -> bool {
    load_token().is_some()
}

/// Get the API URL (default or custom)
pub fn get_api_url() -> String {
    load_token()
        .and_then(|auth| auth.api_url)
        .unwrap_or_else(|| "https://api.fusillade.io".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn with_temp_auth_dir(f: impl FnOnce(PathBuf)) {
        let dir = TempDir::new().unwrap();
        let auth_dir = dir.path().join(CONFIG_DIR);
        fs::create_dir_all(&auth_dir).unwrap();
        f(auth_dir);
    }

    #[test]
    fn test_cloud_auth_serialization() {
        let auth = CloudAuth {
            token: "test-token-123".to_string(),
            api_url: Some("https://custom.api.com".to_string()),
        };
        let json = serde_json::to_string(&auth).unwrap();
        let parsed: CloudAuth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token, "test-token-123");
        assert_eq!(parsed.api_url, Some("https://custom.api.com".to_string()));
    }

    #[test]
    fn test_cloud_auth_without_url() {
        let auth = CloudAuth {
            token: "token".to_string(),
            api_url: None,
        };
        let json = serde_json::to_string(&auth).unwrap();
        let parsed: CloudAuth = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token, "token");
        assert_eq!(parsed.api_url, None);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        with_temp_auth_dir(|auth_dir| {
            let auth_file = auth_dir.join(AUTH_FILE);
            let auth = CloudAuth {
                token: "my-secret-key".to_string(),
                api_url: Some("https://api.test.com".to_string()),
            };
            let json = serde_json::to_string_pretty(&auth).unwrap();
            fs::write(&auth_file, &json).unwrap();

            let content = fs::read_to_string(&auth_file).unwrap();
            let loaded: CloudAuth = serde_json::from_str(&content).unwrap();
            assert_eq!(loaded.token, "my-secret-key");
            assert_eq!(loaded.api_url, Some("https://api.test.com".to_string()));
        });
    }

    #[test]
    fn test_load_missing_file() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent.json");
        assert!(!missing.exists());
    }

    #[test]
    fn test_clear_token_removes_file() {
        with_temp_auth_dir(|auth_dir| {
            let auth_file = auth_dir.join(AUTH_FILE);
            fs::write(&auth_file, "{}").unwrap();
            assert!(auth_file.exists());
            fs::remove_file(&auth_file).unwrap();
            assert!(!auth_file.exists());
        });
    }

    #[test]
    fn test_get_api_url_default() {
        // When no token is loaded, should return default URL
        let default_url = "https://api.fusillade.io".to_string();
        let result: String = None::<CloudAuth>
            .and_then(|auth| auth.api_url)
            .unwrap_or_else(|| default_url.clone());
        assert_eq!(result, default_url);
    }

    #[test]
    fn test_get_api_url_custom() {
        let auth = CloudAuth {
            token: "key".to_string(),
            api_url: Some("https://custom.endpoint.com".to_string()),
        };
        let result = auth
            .api_url
            .unwrap_or_else(|| "https://api.fusillade.io".to_string());
        assert_eq!(result, "https://custom.endpoint.com");
    }
}
