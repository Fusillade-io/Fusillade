# Installation Guide

Fusillade is a high-performance load testing tool written in Rust. You can install it by building from source or using Docker.

## Option 1: Download Binary (Recommended)

You can download the pre-built binaries for Linux, macOS, and Windows from the [Releases page](https://github.com/Fusillade-io/Fusillade/releases).

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
    git clone https://github.com/Fusillade-io/Fusillade.git
    cd Fusillade
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

If you built from source using `cargo install --path .`, both `fusillade` and `fusi` binaries are installed automatically.

If you downloaded a pre-built binary and want a shorter command, you can set up an alias:

**Bash / Zsh:**

Add the following to your `.bashrc` or `.zshrc`:

```bash
alias fusi='fusillade'
```

Usage:

```bash
fusi run scenarios/test.js
```

---

## Platform-Specific Quick Start

### Linux / macOS

```bash
# Download (adjust URL for your platform: linux-x64, macos-x64, macos-arm64)
curl -L https://github.com/Fusillade-io/Fusillade/releases/latest/download/fusillade-linux-x64 -o fusillade
chmod +x fusillade
./fusillade --version
```

### Windows (PowerShell)

```powershell
# Download the Windows binary
Invoke-WebRequest -Uri "https://github.com/Fusillade-io/Fusillade/releases/latest/download/fusillade-windows-x64.exe" -OutFile "fusillade.exe"

# Verify installation
.\fusillade.exe --version
```

---

## Troubleshooting

### Linux: File Descriptor Limits

When running load tests with many concurrent workers, you may hit system file descriptor limits. Increase the limit before running:

```bash
# Check current limit
ulimit -n

# Increase for current session
ulimit -n 65535

# Permanent fix: Add to /etc/security/limits.conf
# * soft nofile 65535
# * hard nofile 65535
```

**Symptoms of hitting limits:**
- "Too many open files" errors
- Connection failures at high concurrency
- Test fails to scale beyond ~1000 workers

### macOS: Too Many Open Files

macOS has a lower default limit. Use:

```bash
sudo launchctl limit maxfiles 65535 200000
ulimit -n 65535
```

### Windows: Connection Limits

Windows may require registry tweaks for high-concurrency testing. See [Microsoft documentation on TCP/IP settings](https://docs.microsoft.com/en-us/windows-server/networking/technologies/network-subsystem/net-sub-performance-tuning-nics).

