use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};

/// Captured request info for replay/export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRequest {
    pub timestamp: String,
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
    pub status: u16,
    pub error: Option<String>,
    pub response_body: Option<String>,
}

impl CapturedRequest {
    /// Convert to cURL command
    pub fn to_curl(&self) -> String {
        let mut parts = vec![format!("curl -X {}", self.method)];

        for (key, value) in &self.headers {
            // Escape quotes in header values
            let escaped_val = value.replace('"', "\\\"");
            parts.push(format!("-H \"{}: {}\"", key, escaped_val));
        }

        if let Some(body) = &self.body {
            if !body.is_empty() {
                let escaped_body = body.replace('"', "\\\"").replace('\n', "\\n");
                parts.push(format!("-d \"{}\"", escaped_body));
            }
        }

        parts.push(format!("'{}'", self.url));
        parts.join(" \\\n  ")
    }
}

/// Append a captured request to the errors file
#[allow(dead_code)]
pub fn capture_failed_request(request: &CapturedRequest, file_path: &str) {
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(file_path) {
        let mut writer = BufWriter::new(file);
        if let Ok(json) = serde_json::to_string(request) {
            let _ = writeln!(writer, "{}", json);
        }
    }
}

/// Load captured requests from file (JSONL format - one JSON per line)
pub fn load_captured_requests(file_path: &str) -> Result<Vec<CapturedRequest>, std::io::Error> {
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);
    let mut requests = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        if let Ok(req) = serde_json::from_str::<CapturedRequest>(&line) {
            requests.push(req);
        }
    }

    Ok(requests)
}

/// Export captured requests to cURL format
pub fn export_to_curl(requests: &[CapturedRequest]) -> String {
    requests
        .iter()
        .enumerate()
        .map(|(i, req)| {
            format!(
                "# Request {} - {} {} (Status: {}, Error: {})\n{}",
                i + 1,
                req.method,
                req.url,
                req.status,
                req.error.as_deref().unwrap_or("none"),
                req.to_curl()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_curl() {
        let req = CapturedRequest {
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            headers: [
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), "Bearer token123".to_string()),
            ]
            .into_iter()
            .collect(),
            body: Some(r#"{"name":"test"}"#.to_string()),
            status: 500,
            error: Some("Internal Server Error".to_string()),
            response_body: Some("Error".to_string()),
        };

        let curl = req.to_curl();
        assert!(curl.contains("curl -X POST"));
        assert!(curl.contains("-H \"Content-Type: application/json\""));
        assert!(curl.contains("-d \"{\\\"name\\\":\\\"test\\\"}\""));
        assert!(curl.contains("'https://api.example.com/users'"));
    }
}
