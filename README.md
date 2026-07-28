# XTR-on-Rust

Rust re-implementation of [buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.1.0 (scaffold — no shipped functionality yet) · **License:** Apache-2.0 · **Author:** Rainer Türner

## Status

**Standards-compliant scaffold + finished domain design.** The
Rust crate, CI pipelines, container hardening posture, publish
workflow, and mdBook documentation are all wired to the same bar
as [Ruuter-on-Rust](https://github.com/turnerrainer/Ruuter). The
**domain design** — what XTR-on-Rust must do — is documented in
[`docs/DESIGN.md`](./docs/DESIGN.md), derived from a direct read
of the original [buerokratt/XTR](https://github.com/buerokratt/XTR):
MVP scope for `v0.2.0-rc.1`, list of correctness fixes over the
JVM version, non-goals, module layout, roadmap.

Implementation of the v0.2.0-rc.1 slice is the next step. See
[`HANDOFF.md`](./HANDOFF.md).

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

- [`docs/DESIGN.md`](./docs/DESIGN.md) — the domain design: what XTR does, what the MVP ships, what's deferred.
- [Book (mdBook)](./book/src/SUMMARY.md) — user-facing reference. Build locally with `mdbook serve book`; browses at `http://localhost:3000`. Auto-deployed to GitHub Pages on push to `main` (see `.github/workflows/docs.yml`).
- [STANDARDS.md](./STANDARDS.md) — every generic build/docs/test/publish rule this project meets.
- [CHANGELOG.md](./CHANGELOG.md)
- Original JVM XTR: <https://github.com/buerokratt/XTR>
