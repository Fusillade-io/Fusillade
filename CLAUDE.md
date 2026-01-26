# Fusillade Development Guide

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

## Quick Pre-Push Checklist

```bash
cargo fmt && cargo clippy && cargo test
```

## Project Structure

- `src/main.rs` - CLI entry point and command handling
- `src/cli/` - Config loading, HAR conversion, validation
- `src/engine/` - Core load testing engine, workers, scheduling
- `src/bridge/` - JS runtime bindings (http, ws, grpc, etc.)
- `src/stats/` - Metrics collection and reporting
- `npm/` - npm package for distribution

## Versioning

When releasing a new version:

1. Update `Cargo.toml` version
2. Update `npm/package.json` version
3. Commit and push
4. Create git tag: `git tag -a vX.Y.Z -m "Release vX.Y.Z"`
5. Push tag: `git push origin vX.Y.Z`
6. Publish npm: `cd npm && npm publish --access public`

## Related Repositories

- **Web**: `saas/web/` - Documentation website
- **Control Plane**: `saas/control-plane/` - Cloud orchestration
- **Data Plane**: `saas/data-plane/` - Worker management

Use the `fusi` SSH key for all git operations.
