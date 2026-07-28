# XTR-on-Rust

Rust re-implementation of [buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.1.0 (scaffold — no shipped functionality yet) · **License:** Apache-2.0 · **Author:** Rainer Türner

## Status

This repo is currently a **standards-compliant scaffold**. The Rust
crate, CI pipelines, container hardening posture, publish workflow,
and mdBook documentation are all wired up to the same bar as
[Ruuter-on-Rust](https://github.com/turnerrainer/Ruuter). Domain
semantics (what XTR actually does) land as follow-up tasks after
the original XTR is analysed. See
[`HANDOFF.md`](./HANDOFF.md) and [`tasks/backlog/`](./tasks/backlog/).

The bar this project meets from day one is documented in
[`STANDARDS.md`](./STANDARDS.md).

## Once published

Multi-arch image (linux/amd64 + linux/arm64) on Docker Hub and GHCR:

```bash
docker run -d --name xtr -p 8080:8080 \
    turnerrainer/xtr:latest
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

- [Book (mdBook)](./book/src/SUMMARY.md) — full LLM-oriented reference. Build locally with `mdbook serve book`; browses at `http://localhost:3000`. Auto-deployed to GitHub Pages on push to `main` (see `.github/workflows/docs.yml`).
- [STANDARDS.md](./STANDARDS.md) — every generic build/docs/test/publish rule this project meets.
- [CHANGELOG.md](./CHANGELOG.md)
- Original JVM XTR: <https://github.com/buerokratt/XTR>
