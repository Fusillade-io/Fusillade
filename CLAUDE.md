# Fusillade Development Guide

## What is Fusillade?

Fusillade is a high-performance load testing platform built in Rust. It combines:
- **Rust** for the engine (networking, threading, metrics)
- **JavaScript** for user test scripts (via QuickJS/rquickjs)

Key design principles:
- OS threads with blocking I/O (not async) for predictable latency
- Metrics capture network time only, excluding JS execution overhead
- Support for HTTP/1.1, HTTP/2, WebSocket, gRPC, MQTT, AMQP, SSE, and browser automation

## Before Pushing Code

Always run these commands before committing/pushing to ensure CI passes:

```bash
# Format code (required - CI will fail if not formatted)
cargo fmt

# Run clippy for lint warnings (fix any errors)
cargo clippy

# Run tests
cargo test
```

### Quick Pre-Push Command

```bash
cargo fmt && cargo clippy && cargo test
```

## Project Structure

```
src/
├── main.rs              # CLI entry point, command handling
├── lib.rs               # Library exports
├── cli/
│   ├── config.rs        # Config parsing (YAML/JSON), options struct
│   ├── har.rs           # HAR file conversion
│   ├── recorder.rs      # HTTP proxy recorder
│   └── validate.rs      # Script validation
├── engine/
│   ├── mod.rs           # Core engine, worker spawning, test execution
│   ├── http_client.rs   # Hyper-based HTTP client with connection pooling
│   ├── distributed.rs   # Distributed mode (worker nodes)
│   ├── control.rs       # Interactive control (pause/resume/ramp)
│   ├── memory.rs        # Memory monitoring and preflight checks
│   └── io_bridge.rs     # Async I/O bridge for blocking workers
├── bridge/
│   ├── http.rs          # JS http module bindings
│   ├── ws.rs            # WebSocket bindings
│   ├── grpc.rs          # gRPC client bindings
│   ├── sse.rs           # Server-Sent Events bindings
│   ├── mqtt.rs          # MQTT client bindings
│   ├── amqp.rs          # AMQP/RabbitMQ bindings
│   ├── browser.rs       # Chromium automation bindings
│   ├── crypto.rs        # Crypto functions (md5, sha1, sha256, hmac)
│   ├── utils.rs         # Utility functions (uuid, random*, etc.)
│   └── replay.rs        # Error replay functionality
├── stats/
│   ├── mod.rs           # Metrics aggregation, histograms
│   └── otlp.rs          # OpenTelemetry export
└── cluster/             # Controller/worker cluster management
```

## Key Files

| File | Purpose |
|------|---------|
| `src/cli/config.rs` | All configuration options, parsing, defaults |
| `src/engine/mod.rs` | Core test execution logic, worker management |
| `src/bridge/http.rs` | HTTP request/response handling, exposed to JS |
| `DOCUMENTATION.md` | Full user documentation (keep in sync with website) |

## Configuration Priority

When running a test, config is merged in this order (later overrides earlier):
1. Script's `export const options = {...}`
2. External config file (`--config file.yaml`)
3. CLI flags (`--workers`, `--duration`, etc.)

## Adding a New JS API

1. Add Rust function in appropriate `src/bridge/*.rs` file
2. Register it in the JS context setup (see `register_*` functions)
3. Add TypeScript types in `fusillade types` output
4. Document in `DOCUMENTATION.md`
5. Update website docs (`saas/web/app/docs/page.tsx`)

## Testing

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test cli::config::tests
cargo test bridge::http::tests
cargo test stats::tests

# Run with output
cargo test -- --nocapture
```

## Versioning & Release

When releasing a new version:

1. Update version in `Cargo.toml`
2. Update version in `npm/package.json`
3. Run linters: `cargo fmt && cargo clippy && cargo test`
4. Commit and push
5. Create git tag: `git tag -a vX.Y.Z -m "Release vX.Y.Z"`
6. Push tag: `git push origin vX.Y.Z`
7. Publish npm: `cd npm && npm publish --access public --otp=<code>`

## Related Repositories

| Repo | Path | Purpose |
|------|------|---------|
| Web | `saas/web/` | Documentation website (Next.js) |
| Control Plane | `saas/control-plane/` | Cloud orchestration service |
| Data Plane | `saas/data-plane/` | Worker management service |

Use the `fusi` SSH key for all git operations:
```bash
eval "$(ssh-agent -s)" && ssh-add ~/.ssh/fusi
```

## Common Tasks

### Add a new CLI flag
1. Add to `Commands::Run` struct in `src/main.rs`
2. Add to destructuring in the match arm
3. Handle the flag in the command logic
4. Document in `DOCUMENTATION.md`

### Add a new config option
1. Add field to `Config` struct in `src/cli/config.rs`
2. Add serde attributes for snake_case/camelCase support
3. Handle in config merging logic (main.rs)
4. Add tests in `config.rs`
5. Document in `DOCUMENTATION.md`

### Add a new HTTP helper
1. Add function in `src/bridge/http.rs`
2. Register in `register_http_module()`
3. Add to response object if needed
4. Document and add to website docs
