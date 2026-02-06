# Build stage
FROM rust:1.83-slim AS builder

WORKDIR /app

# Install protobuf compiler
RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY proto/ ./proto/

# Create dummy src for dependency caching
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs

# Build dependencies only
RUN cargo build --release && rm -rf src

# Copy real source
COPY src/ ./src/

# Build the actual binary
RUN touch src/main.rs src/lib.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/fusillade /usr/local/bin/fusillade

# Create non-root user
RUN useradd -m -u 1000 fusillade
USER fusillade
WORKDIR /home/fusillade

ENTRYPOINT ["fusillade"]
CMD ["--help"]
