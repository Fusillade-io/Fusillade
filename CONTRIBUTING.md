# Contributing to Fusillade

First off, thanks for taking the time to contribute!

The following is a set of guidelines for contributing to Fusillade. These are mostly guidelines, not rules. Use your best judgment, and feel free to propose changes to this document in a pull request.

## How Can I Contribute?

### Reporting Bugs

This section guides you through submitting a bug report. Following these guidelines helps maintainers and the community understand your report, reproduce the behavior, and find related reports.

- **Use a clear and descriptive title** for the issue to identify the problem.
- **Describe the exact steps which reproduce the problem** in as much detail as possible.
- **Provide specific examples** to demonstrate the steps. Include links to files or GitHub projects, or copy/pasteable snippets, which you use in those examples.

### Pull Requests

1.  Fork the repo and create your branch from `main`.
2.  If you've added code that should be tested, add tests.
3.  If you've changed APIs, update the documentation.
4.  Ensure the test suite passes.
5.  Make sure your code lints.

## Development Setup

Fusillade is written in Rust. You will need a working Rust environment.

1.  **Install Rust**: We recommend using [rustup](https://rustup.rs/).
2.  **Install dependencies**:
    - `protoc` (Protocol Buffers compiler) is required for gRPC support.
3.  **Build the project**:
    ```bash
    cargo build
    ```
4.  **Run tests**:
    ```bash
    cargo test
    ```

## Coding Style

- We follow standard Rust coding conventions.
- Please run `cargo fmt` before submitting your PR.
- Please run `cargo clippy` and address any warnings.

## License

By contributing, you agree that your contributions will be licensed under its GNU General Public License v3.0.
