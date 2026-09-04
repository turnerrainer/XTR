FROM rust:1.88-slim AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:13.6-slim
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/xtr-on-rust /app/xtr-on-rust
# Ship the demo self-contained: xtr.yaml + wsdl/ (Ariregister +
# Ministry-of-Climate orbit) + hand-written DSL/xroad/ samples.
# Operators can bind-mount over any of these to override.
COPY xtr.yaml /app/xtr.yaml
COPY wsdl /app/wsdl
COPY DSL /app/DSL

EXPOSE 8080
RUN useradd -m -u 1000 xtr && chown -R xtr:xtr /app
USER xtr

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/xtr-on-rust"]
