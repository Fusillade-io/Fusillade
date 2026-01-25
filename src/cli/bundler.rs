use regex::Regex;
use std::collections::HashMap;
use std::path::Path;

#[allow(dead_code)]
pub fn collect_assets(script_path: &Path) -> HashMap<String, String> {
    let mut assets = HashMap::new();
    let content = match std::fs::read_to_string(script_path) {
        Ok(c) => c,
        Err(_) => return assets,
    };

    // Use r##"..."## to allow quotes and backslashes in regex
    let import_re = Regex::new(r#"from\s+['"](\./.*?)['"]"#).unwrap();
    let open_re = Regex::new(r#"open\s*\(\s*['"](\./.*?)['"]\s*\)"#).unwrap();

    let parent = script_path.parent().unwrap_or(Path::new("."));

    for cap in import_re.captures_iter(&content) {
        let path = &cap[1];
        add_asset(parent, path, &mut assets);
    }

    for cap in open_re.captures_iter(&content) {
        let path = &cap[1];
        add_asset(parent, path, &mut assets);
    }

    assets
}

#[allow(dead_code)]
fn add_asset(parent: &Path, rel_path: &str, assets: &mut HashMap<String, String>) {
    if assets.contains_key(rel_path) {
        return;
    }

    let full_path = parent.join(rel_path);
    if full_path.exists() && full_path.is_file() {
        if let Ok(content) = std::fs::read_to_string(&full_path) {
            assets.insert(rel_path.to_string(), content.clone());

            if rel_path.ends_with(".js") || rel_path.ends_with(".mjs") {
                let nested_assets = scan_content_for_assets(&content);
                for nested_rel in nested_assets {
                    let nested_parent = full_path.parent().unwrap_or(Path::new("."));
                    add_asset(nested_parent, &nested_rel, assets);
                }
            }
        }
    }
}

#[allow(dead_code)]
fn scan_content_for_assets(content: &str) -> Vec<String> {
    let mut results = Vec::new();
    let import_re = Regex::new(r#"from\s+['"](\./.*?)['"]"#).unwrap();
    let open_re = Regex::new(r#"open\s*\(\s*['"](\./.*?)['"]\s*\)"#).unwrap();

    for cap in import_re.captures_iter(content) {
        results.push(cap[1].to_string());
    }
    for cap in open_re.captures_iter(content) {
        results.push(cap[1].to_string());
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_content() {
        let content = r#"
            import { something } from './lib.js';
            const data = open("./data.json");
        "#;
        let assets = scan_content_for_assets(content);
        assert!(assets.contains(&"./lib.js".to_string()));
        assert!(assets.contains(&"./data.json".to_string()));
    }
}
