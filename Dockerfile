FROM rust:1.85-slim-bookworm AS builder
WORKDIR /app

RUN apt-get update && apt-get install -y pkg-config libssl-dev

COPY Cargo.toml Cargo.lock* ./
# Create dummy src to cache dependency compilation
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release 2>/dev/null || true
# Now copy real source and build
COPY src/ src/
RUN touch src/main.rs src/lib.rs
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/lean-agents-rs /usr/local/bin/lean-agents-rs
ENTRYPOINT ["lean-agents-rs"]
