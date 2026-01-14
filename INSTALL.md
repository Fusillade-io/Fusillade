# Installation Guide

Fusillade is a high-performance load testing tool written in Rust. You can install it by building from source or using Docker.

## Option 1: Download Binary (Recommended)

You can download the pre-built binaries for Linux, macOS, and Windows from the [Releases page](https://github.com/yourusername/fusillade/releases).

1.  **Download** the archive for your operating system.
2.  **Extract** the archive.
3.  **Run** the binary directly.

    ```bash
    ./fusillade --version
    ```

## Option 2: Build from Source (Advanced)

This method provides the best performance as the binary is optimized for your specific machine architecture.

### Prerequisites

- **Rust**: Latest stable version.
  - Install via [rustup.rs](https://rustup.rs): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`
- **Protoc**: Protocol Buffers compiler (required for gRPC support).
  - Ubuntu/Debian: `sudo apt-get install protobuf-compiler`
  - macOS: `brew install protobuf`
  - Windows: `choco install protoc`

1.  **Clone the repository:**

    ```bash
    git clone https://github.com/yourusername/fusillade.git
    cd fusillade
    ```

2.  **Build and install:**

    ```bash
    # Install directly to your cargo bin path (e.g., ~/.cargo/bin)
    cargo install --path .
    ```

3.  **Verify installation:**

    ```bash
    fusillade --version
    ```

### Troubleshooting Build Issues

If you encounter errors related to `protoc` or `prost`, ensure you have the Protocol Buffers compiler installed (see Prerequisites).

## Option 3: Docker

You can run Fusillade as a Docker container without installing Rust locally.

1.  **Build the Docker image:**

    ```bash
    docker build -t fusillade .
    ```

2.  **Run a test:**

    Mount your test script into the container to run it.

    ```bash
    # Assuming your test script is in the current directory as 'script.js'
    docker run --rm -v $(pwd):/tests fusillade run /tests/script.js
    ```

## Post-Installation Setup

### Shell Alias

For convenience, you can set up a shorter alias `fusi` for the `fusillade` command.

**Bash / Zsh:**

Add the following to your `.bashrc` or `.zshrc`:

```bash
alias fusi='fusillade'
```

Usage:

```bash
fusi run scenarios/test.js
```

### Shell Completion

To generate shell completion scripts (if supported by your build):

```bash
fusillade completion bash > ~/.fusillade-completion
source ~/.fusillade-completion
```
