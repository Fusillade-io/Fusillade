use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Serialize, Debug)]
struct Har {
    log: Log,
}

#[derive(Deserialize, Serialize, Debug)]
struct Log {
    entries: Vec<Entry>,
}

#[derive(Deserialize, Serialize, Debug)]
struct Entry {
    request: Request,
}

#[derive(Deserialize, Serialize, Debug)]
struct Request {
    method: String,
    url: String,
    headers: Vec<Header>,
    post_data: Option<PostData>,
}

#[derive(Deserialize, Serialize, Debug)]
struct Header {
    name: String,
    value: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct PostData {
    text: Option<String>,
}

pub fn convert_to_js<P: AsRef<Path>>(input: P) -> Result<String> {
    let content = fs::read_to_string(input)?;
    convert_from_string(&content)
}

/// Converts a HAR JSON string to JS script
pub fn convert_from_string(har_content: &str) -> Result<String> {
    let har: Har = serde_json::from_str(har_content)?;

    let mut js = String::new();
    writeln!(&mut js, "export const options = {{")?;
    writeln!(&mut js, "    workers: 1,")?;
    writeln!(&mut js, "    duration: '10s'")?;
    writeln!(&mut js, "}};")?;
    writeln!(&mut js)?;
    writeln!(&mut js, "export default function() {{")?;

    for entry in har.log.entries {
        let req = entry.request;
        writeln!(&mut js, "    http.request({{")?;
        writeln!(&mut js, "        method: '{}',", req.method)?;
        writeln!(&mut js, "        url: '{}',", req.url)?;

        if !req.headers.is_empty() {
            writeln!(&mut js, "        headers: {{")?;
            for header in req.headers {
                let val = header.value.replace("'", "'\\'\'");
                writeln!(&mut js, "            '{}': '{}',", header.name, val)?;
            }
            writeln!(&mut js, "        }},")?;
        }

        if let Some(post) = req.post_data {
            if let Some(text) = post.text {
                let escaped = text.replace("'", "'\\'\'");
                writeln!(&mut js, "        body: '{}',", escaped)?;
            }
        }

        writeln!(&mut js, "    }})")?;
        writeln!(&mut js, "    sleep(100);")?;
    }

    writeln!(&mut js, "}}")?;
    Ok(js)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_har_conversion() {
        let har_json = r#"{
            "log": {
                "entries": [{
                    "request": {
                        "method": "GET",
                        "url": "https://example.com/api",
                        "headers": []
                    }
                }]
            }
        }"#;

        let result = convert_from_string(har_json).unwrap();
        assert!(result.contains("method: 'GET'"));
        assert!(result.contains("url: 'https://example.com/api'"));
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function()"));
    }

    #[test]
    fn test_har_with_headers() {
        let har_json = r#"{
            "log": {
                "entries": [{
                    "request": {
                        "method": "GET",
                        "url": "https://example.com/api",
                        "headers": [
                            {"name": "Content-Type", "value": "application/json"},
                            {"name": "Authorization", "value": "Bearer token123"}
                        ]
                    }
                }]
            }
        }"#;

        let result = convert_from_string(har_json).unwrap();
        assert!(result.contains("headers: {"));
        assert!(result.contains("'Content-Type': 'application/json'"));
        assert!(result.contains("'Authorization': 'Bearer token123'"));
    }

    #[test]
    fn test_har_with_post_data() {
        let har_json = r#"{
            "log": {
                "entries": [{
                    "request": {
                        "method": "POST",
                        "url": "https://example.com/api",
                        "headers": [],
                        "post_data": {
                            "text": "{\"key\": \"value\"}"
                        }
                    }
                }]
            }
        }"#;

        let result = convert_from_string(har_json).unwrap();
        assert!(result.contains("method: 'POST'"));
        assert!(result.contains("body:"));
    }

    #[test]
    fn test_har_empty_entries() {
        let har_json = r#"{
            "log": {
                "entries": []
            }
        }"#;

        let result = convert_from_string(har_json).unwrap();
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function()"));
        // Should not contain any http.request calls
        assert!(!result.contains("http.request"));
    }

    #[test]
    fn test_har_multiple_entries() {
        let har_json = r#"{
            "log": {
                "entries": [
                    {
                        "request": {
                            "method": "GET",
                            "url": "https://example.com/first",
                            "headers": []
                        }
                    },
                    {
                        "request": {
                            "method": "POST",
                            "url": "https://example.com/second",
                            "headers": []
                        }
                    }
                ]
            }
        }"#;

        let result = convert_from_string(har_json).unwrap();
        assert!(result.contains("https://example.com/first"));
        assert!(result.contains("https://example.com/second"));
        // Should have 2 sleep calls
        assert_eq!(result.matches("sleep(100)").count(), 2);
    }

    #[test]
    fn test_har_invalid_json() {
        let invalid_json = "not valid json";
        let result = convert_from_string(invalid_json);
        assert!(result.is_err());
    }
}
