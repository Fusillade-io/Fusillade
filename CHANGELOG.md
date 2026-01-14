# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
