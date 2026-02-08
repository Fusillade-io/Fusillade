use anyhow::Result;
use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer};
use rquickjs::{Ctx, Error, Module};
use std::path::Path;

/// Check if a file path has a TypeScript extension (.ts or .mts)
pub fn is_typescript(path: &str) -> bool {
    let path = Path::new(path);
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts" | "mts")
    )
}

/// Transpile TypeScript source code to JavaScript by stripping types.
/// No type checking is performed — only syntactic type removal.
pub fn transpile_typescript(source: &str, filename: &str) -> Result<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(filename).unwrap_or_default();

    let parsed = Parser::new(&allocator, source, source_type).parse();

    if parsed.panicked {
        let errors: Vec<String> = parsed.errors.iter().map(|e| e.to_string()).collect();
        anyhow::bail!(
            "TypeScript parse error in {}: {}",
            filename,
            errors.join(", ")
        );
    }

    let mut program = parsed.program;

    let semantic = SemanticBuilder::new().build(&program).semantic;

    let options = TransformOptions::default();
    let scoping = semantic.into_scoping();
    let ret = Transformer::new(&allocator, Path::new(filename), &options)
        .build_with_scoping(scoping, &mut program);

    if !ret.errors.is_empty() {
        let errors: Vec<String> = ret.errors.iter().map(|e| e.to_string()).collect();
        anyhow::bail!(
            "TypeScript transform error in {}: {}",
            filename,
            errors.join(", ")
        );
    }

    let output = Codegen::new().build(&program);
    Ok(output.code)
}

/// Transpile source if it's a TypeScript file, otherwise return as-is.
pub fn maybe_transpile(source: String, path: &str) -> Result<String> {
    if is_typescript(path) {
        transpile_typescript(&source, path)
    } else {
        Ok(source)
    }
}

/// A module loader that handles both JavaScript and TypeScript files.
/// TypeScript files are transpiled to JavaScript before being passed to QuickJS.
pub struct TsLoader {
    extensions: Vec<String>,
}

impl Default for TsLoader {
    fn default() -> Self {
        Self {
            extensions: vec!["js".into(), "ts".into(), "mts".into()],
        }
    }
}

impl TsLoader {
    pub fn new() -> Self {
        Self::default()
    }
}

impl rquickjs::loader::Loader for TsLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, path: &str) -> rquickjs::Result<Module<'js>> {
        // Check if extension is one we handle
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !self.extensions.iter().any(|known| known == ext) {
            return Err(Error::new_loading(path));
        }

        let source = std::fs::read_to_string(path).map_err(|_| Error::new_loading(path))?;

        let js_source = if is_typescript(path) {
            transpile_typescript(&source, path)
                .map_err(|e| Error::new_loading_message(path, e.to_string()))?
        } else {
            source
        };

        Module::declare(ctx.clone(), path, js_source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_typescript() {
        assert!(is_typescript("test.ts"));
        assert!(is_typescript("path/to/test.ts"));
        assert!(is_typescript("test.mts"));
        assert!(!is_typescript("test.js"));
        assert!(!is_typescript("test.json"));
        assert!(!is_typescript("test"));
    }

    #[test]
    fn test_transpile_basic_types() {
        let ts = r#"
            const x: number = 42;
            const y: string = "hello";
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("42"));
        assert!(result.contains("hello"));
        assert!(!result.contains(": number"));
        assert!(!result.contains(": string"));
    }

    #[test]
    fn test_transpile_interface() {
        let ts = r#"
            interface Config {
                workers: number;
                duration: string;
            }
            const cfg: Config = { workers: 10, duration: '30s' };
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("workers: 10"));
        assert!(!result.contains("interface Config"));
    }

    #[test]
    fn test_transpile_type_alias() {
        let ts = r#"
            type Id = string | number;
            const id: Id = "abc";
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("\"abc\""));
        assert!(!result.contains("type Id"));
    }

    #[test]
    fn test_transpile_generics() {
        let ts = r#"
            function identity<T>(val: T): T {
                return val;
            }
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("function identity"));
        assert!(result.contains("return val"));
        assert!(!result.contains("<T>"));
    }

    #[test]
    fn test_transpile_enum() {
        let ts = r#"
            enum Color {
                Red,
                Green,
                Blue,
            }
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(!result.contains("enum Color"));
    }

    #[test]
    fn test_transpile_export_with_types() {
        let ts = r#"
            export const options = {
                workers: 10 as number,
                duration: '30s' as string,
            };

            export default function(this: void): void {
                const res = http.get('https://example.com');
            }
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function"));
        assert!(!result.contains("as number"));
        assert!(!result.contains(": void"));
    }

    #[test]
    fn test_maybe_transpile_js_passthrough() {
        let js = "const x = 42;".to_string();
        let result = maybe_transpile(js.clone(), "test.js").unwrap();
        assert_eq!(result, js);
    }

    #[test]
    fn test_maybe_transpile_ts() {
        let ts = "const x: number = 42;".to_string();
        let result = maybe_transpile(ts, "test.ts").unwrap();
        assert!(result.contains("42"));
        assert!(!result.contains(": number"));
    }

    #[test]
    fn test_transpile_invalid_syntax() {
        let ts = "const x: = ;; {{";
        let result = transpile_typescript(ts, "test.ts");
        assert!(result.is_err());
    }

    #[test]
    fn test_transpile_class_with_access_modifiers() {
        let ts = r#"
            class MyService {
                private url: string;
                public timeout: number;
                constructor(url: string, timeout: number = 5000) {
                    this.url = url;
                    this.timeout = timeout;
                }
                async fetch(): Promise<string> {
                    return this.url;
                }
            }
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("class MyService"));
        assert!(!result.contains("private url: string"));
        assert!(!result.contains("public timeout: number"));
        assert!(!result.contains(": Promise<string>"));
    }

    #[test]
    fn test_transpile_arrow_functions_with_types() {
        let ts = r#"
            const add = (a: number, b: number): number => a + b;
            const greet = (name: string): string => `hello ${name}`;
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("=>"));
        assert!(!result.contains(": number =>"));
        assert!(!result.contains(": string =>"));
    }

    #[test]
    fn test_transpile_optional_chaining_with_types() {
        let ts = r#"
            interface Response {
                data?: {
                    users?: Array<{ name: string }>;
                };
            }
            const res: Response = {};
            const name = res.data?.users?.[0]?.name;
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(!result.contains("interface Response"));
        assert!(result.contains("?."));
    }

    #[test]
    fn test_transpile_tuple_types() {
        let ts = r#"
            type Pair = [string, number];
            const pair: Pair = ['hello', 42];
            const [a, b]: [string, number] = pair;
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("pair"));
        assert!(!result.contains("type Pair"));
    }

    #[test]
    fn test_is_typescript_mts() {
        assert!(is_typescript("module.mts"));
        assert!(is_typescript("/path/to/module.mts"));
    }

    #[test]
    fn test_maybe_transpile_mts() {
        let ts = "const x: number = 42;".to_string();
        let result = maybe_transpile(ts, "test.mts").unwrap();
        assert!(result.contains("42"));
        assert!(!result.contains(": number"));
    }

    #[test]
    fn test_transpile_preserves_template_literals() {
        let ts = r#"
            const name: string = "world";
            const msg: string = `hello ${name}`;
        "#;
        let result = transpile_typescript(ts, "test.ts").unwrap();
        assert!(result.contains("`hello ${name}`"));
    }

    #[test]
    fn test_transpile_full_scenario() {
        let ts = r#"
            interface Options {
                workers: number;
                duration: string;
                thresholds: Record<string, string[]>;
            }

            export const options: Options = {
                workers: 10,
                duration: '30s',
                thresholds: {
                    'http_req_duration': ['p95 < 500'],
                },
            };

            function getHeaders(token: string): Record<string, string> {
                return { Authorization: `Bearer ${token}` };
            }

            export default function(): void {
                const headers = getHeaders('test-token');
            }
        "#;
        let result = transpile_typescript(ts, "scenario.ts").unwrap();
        assert!(result.contains("export const options"));
        assert!(result.contains("export default function"));
        assert!(result.contains("function getHeaders"));
        assert!(result.contains("workers: 10"));
        assert!(!result.contains("interface Options"));
        assert!(!result.contains(": Options"));
        assert!(!result.contains(": void"));
        assert!(!result.contains("Record<string, string>"));
    }
}
