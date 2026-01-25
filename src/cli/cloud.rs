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
#[allow(dead_code)]
pub fn clear_token() -> Result<(), std::io::Error> {
    let path = auth_file_path();
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Check if user is logged in
#[allow(dead_code)]
pub fn is_logged_in() -> bool {
    load_token().is_some()
}

/// Get the API URL (default or custom)
pub fn get_api_url() -> String {
    load_token()
        .and_then(|auth| auth.api_url)
        .unwrap_or_else(|| "https://api.fusillade.io".to_string())
}
