FROM rust:1.88-slim AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# Use the codename tag so the base rolls forward on Debian point
# releases (13.6 → 13.7 → …) and picks up security patches
# automatically. A previous Snyk fix pinned to `debian:13.6-slim`
# which then failed a Trivy HIGH/CRITICAL gate at v0.4.0-rc because
# +deb13u1 / +deb13u2 patches for gzip/pcre2/sqlite/perl-base
# weren't in the immutable `13.6-slim` tag. Rolling on `trixie-slim`
# fixes it — CI's Trivy step still gates every publish, so we
# don't lose visibility.
FROM debian:trixie-slim
WORKDIR /app

# `apt-get upgrade` is an extra belt on top of the rolling base
# tag: catches per-package patches that landed after the base
# image was last rebuilt on Docker Hub. Adds ~0-30MB per build
# and a few seconds; well worth the reduced Trivy blast radius.
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
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

# tini as PID 1 forwards signals; the binary is pinned into the
# entrypoint (not CMD) so that `docker run <image> doctor` appends
# `doctor` as an argv to the binary instead of REPLACING it as a
# new exec target. CMD stays empty so bare `docker run <image>`
# still boots the server (no args → server path in main.rs).
ENTRYPOINT ["/usr/bin/tini", "--", "/app/xtr-on-rust"]
CMD []
