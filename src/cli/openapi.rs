use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct OpenApiSpec {
    #[allow(dead_code)]
    pub openapi: String,
    #[allow(dead_code)]
    pub info: Option<Info>,
    pub servers: Option<Vec<Server>>,
    pub paths: BTreeMap<String, PathItem>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Info {
    #[allow(dead_code)]
    pub title: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Server {
    pub url: String,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct PathItem {
    pub get: Option<Operation>,
    pub post: Option<Operation>,
    pub put: Option<Operation>,
    pub delete: Option<Operation>,
    pub patch: Option<Operation>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Operation {
    pub summary: Option<String>,
    pub operation_id: Option<String>,
    pub request_body: Option<RequestBody>,
    pub parameters: Option<Vec<Parameter>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct RequestBody {
    pub content: Option<HashMap<String, MediaType>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct MediaType {
    #[allow(dead_code)]
    pub schema: Option<Schema>,
    pub example: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Schema {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    pub type_field: Option<String>,
    #[allow(dead_code)]
    pub properties: Option<HashMap<String, Schema>>,
    pub example: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct Parameter {
    pub name: String,
    #[serde(rename = "in")]
    #[allow(dead_code)]
    pub in_field: String,
    #[allow(dead_code)]
    pub required: Option<bool>,
    #[allow(dead_code)]
    pub schema: Option<Schema>,
    pub example: Option<serde_json::Value>,
}

pub fn convert_to_js<P: AsRef<Path>>(input: P) -> Result<String> {
    let path = input.as_ref();
    let content = fs::read_to_string(path)?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let spec: OpenApiSpec = match ext {
        "json" => serde_json::from_str(&content)?,
        "yaml" | "yml" => serde_yaml::from_str(&content)?,
        _ => {
            // Try JSON first, then YAML
            if let Ok(s) = serde_json::from_str(&content) {
                s
            } else {
                serde_yaml::from_str(&content)?
            }
        }
    };

    convert_from_spec(spec)
}

/// Converts an already-parsed OpenAPI string (YAML) into JS script.
/// Useful for testing without files.
pub fn convert_from_yaml_string(yaml_content: &str) -> Result<String> {
    let spec: OpenApiSpec = serde_yaml::from_str(yaml_content)?;
    convert_from_spec(spec)
}

fn convert_from_spec(spec: OpenApiSpec) -> Result<String> {
    let base_url = spec
        .servers
        .as_ref()
        .and_then(|servers| servers.first())
        .map(|s| s.url.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "https://api.example.com".to_string());

    let mut js = String::new();

    writeln!(&mut js, "export const options = {{")?;
    writeln!(&mut js, "    workers: 1,")?;
    writeln!(&mut js, "    duration: '10s'")?;
    writeln!(&mut js, "}};")?;
    writeln!(&mut js)?;
    writeln!(&mut js, "const BASE_URL = '{}';", base_url)?;
    writeln!(&mut js)?;
    writeln!(&mut js, "export default function() {{")?;

    for (path, item) in &spec.paths {
        let methods: Vec<(&str, Option<&Operation>)> = vec![
            ("GET", item.get.as_ref()),
            ("POST", item.post.as_ref()),
            ("PUT", item.put.as_ref()),
            ("DELETE", item.delete.as_ref()),
            ("PATCH", item.patch.as_ref()),
        ];

        for (method, op) in methods {
            if let Some(operation) = op {
                // Comment line
                let default_comment = format!("{} {}", method, path);
                let comment = operation
                    .summary
                    .as_deref()
                    .or(operation.operation_id.as_deref())
                    .unwrap_or(&default_comment);
                writeln!(&mut js, "    // {}", comment)?;

                // Resolve path with parameters
                let resolved_path = resolve_path_params(path, operation);

                match method {
                    "GET" => {
                        writeln!(
                            &mut js,
                            "    http.get(BASE_URL + '{}', {{ headers: {{'Content-Type': 'application/json'}} }});",
                            resolved_path
                        )?;
                    }
                    "DELETE" => {
                        writeln!(&mut js, "    http.del(BASE_URL + '{}');", resolved_path)?;
                    }
                    _ => {
                        // POST, PUT, PATCH
                        let body = extract_example_body(operation);
                        writeln!(&mut js, "    http.request({{")?;
                        writeln!(&mut js, "        method: '{}',", method)?;
                        writeln!(&mut js, "        url: BASE_URL + '{}',", resolved_path)?;
                        writeln!(
                            &mut js,
                            "        headers: {{'Content-Type': 'application/json'}},"
                        )?;
                        writeln!(&mut js, "        body: JSON.stringify({})", body)?;
                        writeln!(&mut js, "    }});")?;
                    }
                }

                writeln!(&mut js, "    sleep(0.1);")?;
            }
        }
    }

    writeln!(&mut js, "}}")?;
    Ok(js)
}

/// Replace `{param}` in a path with example values from parameters if available.
fn resolve_path_params(path: &str, operation: &Operation) -> String {
    let mut resolved = path.to_string();
    if let Some(params) = &operation.parameters {
        for param in params {
            let placeholder = format!("{{{}}}", param.name);
            if resolved.contains(&placeholder) {
                let value = param
                    .example
                    .as_ref()
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| param.name.clone());
                let replacement = format!("${{{}}}", value);
                resolved = resolved.replace(&placeholder, &replacement);
            }
        }
    }
    resolved
}

/// Extract an example body from the request body definition.
fn extract_example_body(operation: &Operation) -> String {
    if let Some(body) = &operation.request_body {
        if let Some(content) = &body.content {
            // Try application/json first, then any content type
            let media = content
                .get("application/json")
                .or_else(|| content.values().next());
            if let Some(media_type) = media {
                if let Some(example) = &media_type.example {
                    return serde_json::to_string(example).unwrap_or_else(|_| "{}".to_string());
                }
                if let Some(schema) = &media_type.schema {
                    if let Some(example) = &schema.example {
                        return serde_json::to_string(example).unwrap_or_else(|_| "{}".to_string());
                    }
                }
            }
        }
    }
    "{}".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_openapi_conversion() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Test API"
paths:
  /health:
    get:
      summary: "Health check"
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function()"));
        assert!(result.contains("http.get(BASE_URL + '/health'"));
        assert!(result.contains("// Health check"));
        assert!(result.contains("sleep(0.1)"));
        // Default base URL since no servers specified
        assert!(result.contains("const BASE_URL = 'https://api.example.com'"));
    }

    #[test]
    fn test_openapi_with_post() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Test API"
paths:
  /users:
    post:
      summary: "Create user"
      requestBody:
        content:
          application/json:
            example:
              name: "John"
              email: "john@example.com"
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("method: 'POST'"));
        assert!(result.contains("// Create user"));
        assert!(result.contains("JSON.stringify("));
        assert!(result.contains("\"name\":\"John\""));
        assert!(result.contains("\"email\":\"john@example.com\""));
    }

    #[test]
    fn test_openapi_with_servers() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Test API"
servers:
  - url: "https://my-api.example.com/v1"
paths:
  /ping:
    get:
      summary: "Ping"
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("const BASE_URL = 'https://my-api.example.com/v1'"));
    }

    #[test]
    fn test_openapi_with_path_params() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Test API"
paths:
  /users/{id}:
    get:
      summary: "Get user by ID"
      parameters:
        - name: id
          in: path
          required: true
          example: 42
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("${42}"));
        assert!(!result.contains("{id}"));
    }

    #[test]
    fn test_openapi_multiple_methods() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Test API"
paths:
  /items:
    get:
      summary: "List items"
    post:
      summary: "Create item"
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("// List items"));
        assert!(result.contains("// Create item"));
        assert!(result.contains("http.get("));
        assert!(result.contains("method: 'POST'"));
        assert_eq!(result.matches("sleep(0.1)").count(), 2);
    }

    #[test]
    fn test_openapi_empty_paths() {
        let yaml = r#"
openapi: "3.0.0"
info:
  title: "Empty API"
paths: {}
"#;
        let result = convert_from_yaml_string(yaml).unwrap();
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function()"));
        assert!(!result.contains("http.get"));
        assert!(!result.contains("http.request"));
        assert!(!result.contains("http.del"));
    }

    #[test]
    fn test_openapi_invalid_yaml() {
        let invalid = "{{{{not valid yaml at all::::";
        let result = convert_from_yaml_string(invalid);
        assert!(result.is_err());
    }
}
