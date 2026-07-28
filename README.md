# XTR-on-Rust

Rust re-implementation of [buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.2.0-rc.1 (first publishable release candidate) · **License:** Apache-2.0 · **Author:** Rainer Türner

## Status

**Working REST → SOAP → X-Road proxy**, live-verified against
public Ariregister endpoints. MVP per
[`docs/DESIGN.md`](./docs/DESIGN.md) §8 delivered by task 002.
Post-MVP hardening sweep landed tasks 003 / 005 / 010 / 011 / 012:
SOAP Fault detection (200 *and* non-2xx), request+response size
caps, opt-in JSON type coercion, explicit X-Road protocol
version, quick-xml CVE upgrade with XXE guard, XML nesting-depth
cap, DSL loader startup-time Handlebars validation, OpenAPI
error-response schema.

CI pipelines, container hardening posture, publish workflow, and
mdBook documentation match the [Ruuter-on-Rust](https://github.com/turnerrainer/Ruuter) bar.

Test count: **58 passed / 0 failed / 0 ignored**.
`cargo audit`: 0 advisories. `cargo deny check`: green.

The bar this project meets from day one is documented in
[`STANDARDS.md`](./STANDARDS.md).

## Once published

Multi-arch image (linux/amd64 + linux/arm64) on Docker Hub and GHCR:

```bash
docker run -d --name xtr -p 8080:8080 \
    turnerrainer/xtr:0.2.0-rc.1
```

Every published digest is signed keyless via cosign — verify with the
recipe in [book/src/ops/docker.md](book/src/ops/docker.md#verify-the-image-cosign-once-published).

## Build from source

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
docker compose up -d --build
```

## Documentation

- [`docs/DESIGN.md`](./docs/DESIGN.md) — the domain design: what XTR does, what the MVP ships, what's deferred.
- [Book (mdBook)](./book/src/SUMMARY.md) — user-facing reference. Build locally with `mdbook serve book`; browses at `http://localhost:3000`. Auto-deployed to GitHub Pages on push to `main` (see `.github/workflows/docs.yml`).
- [STANDARDS.md](./STANDARDS.md) — every generic build/docs/test/publish rule this project meets.
- [CHANGELOG.md](./CHANGELOG.md)
- Original JVM XTR: <https://github.com/buerokratt/XTR>
