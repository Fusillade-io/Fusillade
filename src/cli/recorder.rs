use anyhow::Result;
use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::any,
    Router,
};
use reqwest::Client;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

pub(crate) struct RecordedRequest {
    method: String,
    url: String,
    headers: std::collections::HashMap<String, String>,
    body: Option<String>,
}

#[derive(Clone)]
struct RecorderState {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    client: Client,
}

pub async fn run_recorder(output: PathBuf, port: u16) -> Result<()> {
    let state = RecorderState {
        requests: Arc::new(Mutex::new(Vec::new())),
        client: Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?,
    };

    let app = Router::new()
        .fallback(any(handle_proxy))
        .with_state(state.clone());

    println!("Flow Recorder listening on http://0.0.0.0:{}", port);
    println!("Configure your application to use this as a proxy or hit it directly.");
    println!("Press Ctrl+C to stop and save the flow.");

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    let server_task = axum::serve(listener, app);

    tokio::select! {
        _ = server_task => {},
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping recorder and saving flow...");
        }
    }

    save_flow(&output, &state.requests.lock().unwrap())?;
    println!("Flow saved to {:?}", output);

    Ok(())
}

async fn handle_proxy(State(state): State<RecorderState>, req: Request) -> Response {
    let method = req.method().clone();
    let url_str = req.uri().to_string();

    let mut captured_body = None;
    let (parts, body) = req.into_parts();

    let mut headers = std::collections::HashMap::new();
    for (name, value) in &parts.headers {
        match value.to_str() {
            Ok(v) => {
                headers.insert(name.to_string(), v.to_string());
            }
            Err(_) => {
                eprintln!(
                    "[Recorder] Warning: non-UTF-8 header value for '{}', skipping",
                    name
                );
            }
        }
    }

    let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .unwrap_or_default();
    if !body_bytes.is_empty() {
        captured_body = Some(String::from_utf8_lossy(&body_bytes).to_string());
    }

    {
        let mut reqs = state.requests.lock().unwrap();
        reqs.push(RecordedRequest {
            method: method.to_string(),
            url: url_str.clone(),
            headers: headers.clone(),
            body: captured_body.clone(),
        });
    }

    let target_url = if url_str.starts_with("http") {
        url_str
    } else {
        return Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from(
                "Recorder requires absolute URLs (configure as Forward Proxy)",
            ))
            .unwrap();
    };

    let mut builder = state.client.request(method, &target_url);
    for (k, v) in headers {
        if k.to_lowercase() != "host" && k.to_lowercase() != "content-length" {
            builder = builder.header(k, v);
        }
    }
    if let Some(b) = captured_body {
        builder = builder.body(b);
    }

    match builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            let mut res_builder = Response::builder().status(status);
            for (k, v) in resp.headers() {
                res_builder = res_builder.header(k, v);
            }
            let bytes = resp.bytes().await.unwrap_or_default();
            res_builder.body(Body::from(bytes)).unwrap()
        }
        Err(e) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::from(format!("Proxy Error: {}", e)))
            .unwrap(),
    }
}

pub(crate) fn save_flow(output: &PathBuf, requests: &[RecordedRequest]) -> Result<()> {
    let mut js = String::new();
    // Fusillade provides http, check, sleep as globals

    js.push_str("export const options = {\n");
    js.push_str("    workers: 1,\n");
    js.push_str("    duration: '10s',\n");
    js.push_str("};\n\n");
    js.push_str("export default function() {\n");

    for req in requests {
        js.push_str("    http.request({\n");
        js.push_str(&format!("        method: '{}',\n", req.method));
        js.push_str(&format!("        url: '{}',\n", req.url));
        if let Some(body) = &req.body {
            js.push_str(&format!(
                "        body: {},\n",
                serde_json::to_string(body).unwrap()
            ));
        }
        if !req.headers.is_empty() {
            js.push_str("        headers: {\n");
            for (k, v) in &req.headers {
                if k.to_lowercase() != "content-length" && k.to_lowercase() != "host" {
                    let escaped_v = v.replace('\\', "\\\\").replace('\'', "\\'");
                    js.push_str(&format!("            '{}': '{}',\n", k, escaped_v));
                }
            }
            js.push_str("        },\n");
        }
        js.push_str("    });\n");
        js.push_str("    sleep(1);\n\n");
    }

    js.push_str("}\n");

    std::fs::write(output, js)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_request(method: &str, url: &str) -> RecordedRequest {
        RecordedRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: std::collections::HashMap::new(),
            body: None,
        }
    }

    #[test]
    fn test_save_flow_basic() {
        let tmp = NamedTempFile::new().unwrap();
        let requests = vec![make_request("GET", "https://api.example.com/users")];
        save_flow(&tmp.path().to_path_buf(), &requests).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("export const options"));
        assert!(content.contains("export default function()"));
        assert!(content.contains("method: 'GET'"));
        assert!(content.contains("url: 'https://api.example.com/users'"));
    }

    #[test]
    fn test_save_flow_multiple_requests() {
        let tmp = NamedTempFile::new().unwrap();
        let requests = vec![
            make_request("GET", "https://api.example.com/users"),
            make_request("POST", "https://api.example.com/users"),
        ];
        save_flow(&tmp.path().to_path_buf(), &requests).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert_eq!(content.matches("http.request(").count(), 2);
        assert!(content.contains("sleep(1)"));
    }

    #[test]
    fn test_save_flow_post_with_body() {
        let tmp = NamedTempFile::new().unwrap();
        let requests = vec![RecordedRequest {
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            headers: std::collections::HashMap::new(),
            body: Some(r#"{"name":"test"}"#.to_string()),
        }];
        save_flow(&tmp.path().to_path_buf(), &requests).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("body:"));
        assert!(content.contains("name"));
    }

    #[test]
    fn test_save_flow_custom_headers() {
        let tmp = NamedTempFile::new().unwrap();
        let mut headers = std::collections::HashMap::new();
        headers.insert("authorization".to_string(), "Bearer token123".to_string());
        headers.insert("x-custom".to_string(), "value".to_string());

        let requests = vec![RecordedRequest {
            method: "GET".to_string(),
            url: "https://api.example.com/data".to_string(),
            headers,
            body: None,
        }];
        save_flow(&tmp.path().to_path_buf(), &requests).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("authorization"));
        assert!(content.contains("Bearer token123"));
    }

    #[test]
    fn test_save_flow_empty() {
        let tmp = NamedTempFile::new().unwrap();
        save_flow(&tmp.path().to_path_buf(), &[]).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(content.contains("export default function()"));
        assert!(!content.contains("http.request("));
    }

    #[test]
    fn test_save_flow_escapes_special_chars() {
        let tmp = NamedTempFile::new().unwrap();
        let mut headers = std::collections::HashMap::new();
        headers.insert("x-test".to_string(), "it's a \"test\"".to_string());

        let requests = vec![RecordedRequest {
            method: "GET".to_string(),
            url: "https://api.example.com".to_string(),
            headers,
            body: None,
        }];
        save_flow(&tmp.path().to_path_buf(), &requests).unwrap();

        let content = std::fs::read_to_string(tmp.path()).unwrap();
        // Single quotes in header values should be escaped
        assert!(content.contains("it\\'s"));
    }
}
