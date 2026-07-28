# HANDOFF

**Written**: 2026-07-28
**Last verified green**: 2026-07-28 — cargo test 58/0/0
(47 unit + 11 integration); fmt + clippy -D warnings clean;
cargo audit clean (0 advisories); cargo deny check clean.
Live smoke test loads all 6 shipped DSL samples, serves
`/health` + `/api` (with the enhanced XtrError schema), and
handles a real Ariregister SOAP Fault as `upstream_soap_fault`.
**Branch**: `dev` — task 002 done, hardening sweep landed
(tasks 003, 005, 010, 011, 012 + follow-ups). `v0.2.0-rc.1`
release-prep landed 2026-07-28; awaits tag push.
**Release**: `v0.2.0-rc.1` prepared. Cargo.toml, VERSION,
docker-compose.yml, README, book, HANDOFF, CHANGELOG all
bumped. Tag push is the operator's move.

This file is the entry point for the next contributor (human or
agent). Read this, run the first-run checklist, then dive into the
specific files it points at.

## What this repo IS today

A **working REST → SOAP → X-Road proxy in Rust**, plus the
standards-compliant scaffold + CI + publish pipeline for it.
Every rule from Ruuter-on-Rust's hardening cycle applies from
day one — see [`STANDARDS.md`](./STANDARDS.md).

Currently implements (per DESIGN.md §8):

- `POST /:group/:service` — DSL lookup → Handlebars expand →
  executor (plain HTTPS or mTLS to X-Road Security Server) →
  XML → JSON translation → response
- `GET /health` — `{"status":"ok"}`
- `GET /api` — auto-generated OpenAPI 3.1 from loaded DSLs
- 12 of the 17 JVM XTR bugs from DESIGN.md §7 fixed
  (subsystem_code typo, single-pass Handlebars, system trust
  store, structured errors, /:group/:service route, /health
  endpoint, port 8080 everywhere, OpenAPI schema type
  "string" not "String", etc.)
- 6 shipped DSL samples imported from JVM XTR

## What this repo does NOT have yet

- Published container image — the publish workflow is wired up
  and passes locally; ships on `git push origin v0.2.0-rc.1`
  after phase J of task 002.
- Full Docker Hub / GHCR setup — see "Before the first tag
  push" below (one-time operator setup).

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
   [`docs/DESIGN.md`](./docs/DESIGN.md).
2. ✅ **Task 002 (done)**: implemented the v0.2.0-rc.1 MVP
   slice per DESIGN.md §8. 10 phases (A–J), 29 tests, 12 of
   17 JVM bugs fixed. See
   [`tasks/done/002-implement-mvp-v0.2.0-rc.1.md`](./tasks/done/002-implement-mvp-v0.2.0-rc.1.md).
3. **Task 009 — release prep**: metadata bump (Cargo.toml,
   VERSION, CHANGELOG, docker-compose image tag, docs) to
   flip to `0.2.0-rc.1`. Precedes the first tag push.
4. **First release cut**: `v0.2.0-rc.1` after task 009 merges.
5. **Iterate** in `0.x` line on `dev`. `main` reserved for
   `v1.0.0`. Post-MVP work is organised into **epics** under
   `tasks/backlog/epic-*/` (see below). v0.3+ roadmap lives
   in [`docs/DESIGN.md`](./docs/DESIGN.md#9-roadmap-beyond-mvp).

## Open backlog

| Task | Location | Status |
|---|---|---|
| **009** — release prep for v0.2.0-rc.1 | [`tasks/backlog/009-release-prep-v0.2.0-rc.1.md`](./tasks/backlog/009-release-prep-v0.2.0-rc.1.md) | Next up |
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
