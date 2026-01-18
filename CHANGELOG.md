# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.9] - 2026-01-18

### Changed
- **Release Assets**: Binaries are now released as raw executables (no tar.gz/zip compression).

## [0.5.8] - 2026-01-18

### Changed
- **Release Assets**: Renamed binaries from `darwin-amd64` to `macos-x64` and `amd64` to `x64` for consistency.

## [0.5.7] - 2026-01-18

### Added
- **k6 Migration Guide**: New section in documentation with step-by-step instructions for migrating from k6.
- **Windows PowerShell Install**: Added PowerShell download commands to INSTALL.md.
- **Linux/macOS Troubleshooting**: Added file descriptor limit guidance for high-concurrency testing.

### Fixed
- **Clippy Warnings**: Resolved 2 style warnings in test code.

## [0.5.6] - 2026-01-17

### Added
- **Status Code Breakdown**: Summary now includes HTTP status code counts (e.g., 200: 5000, 500: 12) sent to metrics endpoint at test completion.

## [0.5.5] - 2026-01-17

### Added
- **`--vus` Alias**: Added `--vus` as an alias for `--workers` for k6 compatibility.
- **`--headless` Flag**: Restored `--headless` flag for CI/CD mode. TUI is now the default when running locally.
- **`--no-endpoint-tracking` Flag**: New option to disable per-URL metrics collection, reducing memory usage for high-cardinality URL patterns.

## [0.5.4] - 2026-01-17

### Added
- **Endpoint Metrics**: Per-endpoint breakdown metrics sent at test completion. Tracks request count, average latency, p95 latency, min/max latency, and error count for each endpoint.

## [0.5.2] - 2026-01-16

### Added
- **`fusillade init`**: New command to create starter test scripts with templates.
- **`fusillade validate`**: New command to validate scripts without running (checks syntax, config).
- **`fusillade completion`**: Generate shell completions for bash, zsh, fish, and powershell.

## [0.5.1] - 2026-01-15

### Added
- **Response Sink Mode**: New `response_sink` option to discard response bodies and save memory during high-throughput tests. Body is still downloaded from the network (required for connection keep-alive) but immediately discarded instead of being stored. Supports both `response_sink` (snake_case) and `responseSink` (camelCase) in config.

## [0.5.0] - 2026-01-14

### Added
- **Metric Timing Audit**: Improved verification of latency metrics.
- **Headless Mode**: Support for running without UI for CI/CD environments.
- **Bridge Profiling**: Enhanced internal profiling capabilities.
- **Distributed Aggregation**: Support for aggregating metrics from distributed workers.
- **Windows Support**: Full support for building and running on Windows.
- **Binary Releases**: Automated GitHub Actions workflow to release binaries for Linux, macOS, and Windows.
- **`fusi` Alias**: Shorter command alias for improved developer experience.

### Changed
- **Renamed Project**: Officially renamed from "Thruster" to "Fusillade".
- **Documentation**: Overhauled installation and usage documentation.
- **Controller**: Now supports a configurable listen address.

### Fixed
- **Performance**: Resolved performance bottlenecks in high-concurrency scenarios.
- **Panic on Shutdown**: Fixed `no reactor running` panic by improving lifecycle management.
