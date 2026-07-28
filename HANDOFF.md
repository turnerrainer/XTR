# HANDOFF

**Written**: 2026-07-28
**Last verified green**: N/A — scaffold only, no CI runs yet.
**Branch**: `dev` (initial commit).
**Release**: 0.1.0 (Cargo.toml + CHANGELOG dated 2026-07-28).

This file is the entry point for the next contributor (human or
agent). Read this, run the first-run checklist, then dive into the
specific files it points at.

## What this repo IS today

A **standards-compliant scaffold** for the Rust re-implementation of
[buerokratt/XTR](https://github.com/buerokratt/XTR). Every rule from
Ruuter-on-Rust's hardening cycle is applied from day one — see
[`STANDARDS.md`](./STANDARDS.md).

## What this repo does NOT have yet

- Domain code — the Rust binary prints a placeholder and exits.
- Published container image — the publish workflow is wired up but
  will fire the first time on `git push origin v0.2.0-rc.1` (or
  whatever tag) after the first real domain feature lands.
- Test suite — `cargo test` returns 0 pass / 0 fail. Grows with
  each feature.

## First-run checklist

1. **Open a fresh shell** in `/home/rainer/Desktop/Buerostack/XTR`.
2. **Run the verification set** — this is the ceiling of what
   scaffold can prove; every command below should exit 0:
   ```bash
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo build --release --bin xtr-on-rust
   cargo test --no-fail-fast
   cargo audit --deny warnings          # needs `cargo install cargo-audit`
   ( cd book && mdbook build )
   ```
3. **Read [`STANDARDS.md`](./STANDARDS.md)** front-to-back. That's
   the contract every future change follows.
4. **Read [`docs/DESIGN.md`](./docs/DESIGN.md)** — what
   XTR-on-Rust must do. Produced by task 001.

## Roadmap (planned order)

1. ✅ **Task 001 (done)**: analysed original XTR (JVM). See
   [`docs/DESIGN.md`](./docs/DESIGN.md) — MVP scope, fixes,
   non-goals, module layout, roadmap.
2. **Task 002 — the first domain PR**: implement the
   v0.2.0-rc.1 slice per `docs/DESIGN.md` §8. Deliverables:
   - `GET /health` returning `{"status":"ok"}`
   - `POST /:group/:service` routing to a Handlebars-expanded
     SOAP envelope, sent either plain-HTTPS or via mTLS to the
     Security Server
   - `GET /api` — auto-generated OpenAPI 3.1 from loaded DSLs
   - Integration tests using shipped DSL samples + mock upstream
3. **First release cut**: `v0.2.0-rc.1` once task 002 ships
   with tests. Same PR-flow-to-`dev`-then-tag process as
   Ruuter-on-Rust.
4. **Iterate** in `0.x` line on `dev`. `main` reserved for
   `v1.0.0`. Post-MVP work is organised into **epics** under
   `tasks/backlog/epic-*/` (see below). v0.3+ roadmap lives
   in [`docs/DESIGN.md`](./docs/DESIGN.md#9-roadmap-beyond-mvp).

## Open backlog

| Task | Location | Status |
|---|---|---|
| **002** — implement MVP v0.2.0-rc.1 | [`tasks/backlog/002-implement-mvp-v0.2.0-rc.1.md`](./tasks/backlog/002-implement-mvp-v0.2.0-rc.1.md) | Next up |
| **Epic — X-Road protocol compliance** | [`tasks/backlog/epic-xroad-protocol-compliance/`](./tasks/backlog/epic-xroad-protocol-compliance/) | 3 open tasks (003, 004, 005) |
| **Epic — Operator onboarding** | [`tasks/backlog/epic-operator-onboarding/`](./tasks/backlog/epic-operator-onboarding/) | 1 open task (006) |
| **Epic — Testing infrastructure** | [`tasks/backlog/epic-testing-infrastructure/`](./tasks/backlog/epic-testing-infrastructure/) | 2 open tasks (007, 008) |

Each epic directory has its own `README.md` explaining scope +
closing criteria. See STANDARDS.md §13 for the epic filing
convention.

## Before the first tag push

Same one-time setup as Ruuter-on-Rust required (see STANDARDS.md §11):

1. Create `turnerrainer/xtr` repository on Docker Hub (empty is fine).
2. Repo Actions permissions: Settings → Actions → General →
   Workflow permissions → **"Read and write permissions"**.
3. Repo secrets (Settings → Secrets and variables → Actions):
   - `DOCKERHUB_USERNAME` — Docker Hub account
   - `DOCKERHUB_TOKEN` — access token with Read/Write/Delete scope
4. First GHCR push: after it lands, go to Package settings → Manage
   Actions access → link XTR repo with Write role. (Personal-
   namespace packages need this before `GITHUB_TOKEN` can push
   subsequent versions.)

## Where to look for more detail

| Topic | File |
|---|---|
| **Domain design — what XTR-on-Rust must do (v0.2.0-rc.1 MVP scope + roadmap)** | [`docs/DESIGN.md`](./docs/DESIGN.md) |
| Every generic rule this project follows | [`STANDARDS.md`](./STANDARDS.md) |
| CI security gate | [`.github/workflows/security.yml`](./.github/workflows/security.yml), [`deny.toml`](./deny.toml), [`.cargo/audit.toml`](./.cargo/audit.toml) |
| Publish workflow | [`.github/workflows/publish.yml`](./.github/workflows/publish.yml) |
| Private security disclosure | [`SECURITY.md`](./SECURITY.md) |
| Full change history | [`CHANGELOG.md`](./CHANGELOG.md) |
