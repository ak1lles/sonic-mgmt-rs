# -- builder --
FROM rust:1.75-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin sonic-mgmt

# -- runtime --
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssh-client iputils-ping iproute2 curl jq ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/sonic-mgmt /usr/local/bin/sonic-mgmt
WORKDIR /workspace
ENTRYPOINT ["sonic-mgmt"]
