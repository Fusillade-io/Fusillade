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

struct RecordedRequest {
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
        headers.insert(name.to_string(), value.to_str().unwrap_or("").to_string());
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

fn save_flow(output: &PathBuf, requests: &[RecordedRequest]) -> Result<()> {
    let mut js = String::new();
    // Fusillade provides http, check, sleep as globals

    js.push_str("export const options = {\n");
    js.push_str("    vus: 1,\n");
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
                    let escaped_v = v.replace("'", "'\\");
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
