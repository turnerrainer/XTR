# HANDOFF

**Written**: 2026-07-29
**Last verified green**: 2026-07-29 — cargo test 82/0/0
(71 unit + 11 integration); fmt + clippy -D warnings clean;
cargo audit clean (0 advisories); cargo deny check clean;
mdbook + linkcheck build clean. Default boot (with shipped
`wsdl/ar/`) takes ~1 second, materialises 35 endpoints
(33 auto-generated Ariregister + 2 hand-written X-Road).
**Branch**: `dev` — task 002 done, hardening sweep landed
(tasks 003, 005, 010, 011, 012), task 013 (WSDL folder-drop)
landed, task 006 (Security Server onboarding docs) landed.
Task 014 (loader scale optimization) filed. `v0.1.0-rc.2`
tag exists **locally** (2 commits ahead of tag: harvest
script + linkcheck fix). Not pushed yet.
**Release**: `v0.1.0-rc.2` locally tagged at `9434245`. HEAD
(`7b03f97`) is the current-truth working state. Whether to
move the tag or ship as-is is operator's call.

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
- **WSDL folder-drop** (task 013) — `wsdl_watch_dir:` config
  field. XTR ingests `<wsdl_watch_dir>/<group>/*.wsdl` at boot,
  parses each with the in-tree SOAP-1.1 parser
  (`src/wsdl/`), and generates `DSL/<group>/<operation>.yml`
  per operation. Marker-header collision rules: hand-written
  DSLs (no marker) always win.
- **Ariregister shipped as default** — `wsdl/ar/ariregister.wsdl`
  + 34 companion XSDs bundled in the repo. Default `xtr.yaml`
  points `wsdl_watch_dir` at `./wsdl`, yielding 33
  auto-generated `/ar/*` endpoints on boot.
- **Estonian catalog harvest script** —
  `scripts/harvest-xtee-wsdls.sh` fetches ALL public WSDLs
  from RIA's x-tee.ee catalog (~637 WSDLs, ~4300 methods).
  Fully reproducible; catalog data itself is NOT committed
  (see task 014 for the scale caveat).
- 12 of the 17 JVM XTR bugs from DESIGN.md §7 fixed
  (subsystem_code typo, single-pass Handlebars, system trust
  store, structured errors, /:group/:service route, /health
  endpoint, port 8080 everywhere, OpenAPI schema type
  "string" not "String", etc.)

## What this repo does NOT have yet

- Published container image — the publish workflow is wired up
  and passes locally; ships on `git push origin v0.1.0-rc.2`
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

Landed 2026-07-28 through 2026-07-29:

- ✅ **Task 001** — analysed original XTR (JVM). See
  [`docs/DESIGN.md`](./docs/DESIGN.md).
- ✅ **Task 002** — MVP per DESIGN.md §8 (working proxy).
- ✅ **Task 003** — Content-Type + charset (closed as
  landed with 002 Phase D).
- ✅ **Task 005** — X-Road protocol version in config.
- ✅ **Task 006** — Security Server onboarding docs.
- ✅ **Task 009** — release prep for v0.2.0-rc.1 (then
  bumped to rc.2 with WSDL folder-drop landing).
- ✅ **Task 010** — SOAP Fault detection (200 + non-2xx).
- ✅ **Task 011** — request/response size caps + timeout.
- ✅ **Task 012** — opt-in JSON type coercion.
- ✅ **Task 013** — WSDL folder-drop + auto-generation.
  Ariregister ships as default demo.
- Security sweep — quick-xml CVE upgrade, XXE guard,
  nesting-depth cap, regression coverage.

Next up:

- **Task 004** — response requestHash verification
  (needs a real Security Server or task 007's mock).
- **Task 007** — mock X-Road Security Server for CI.
- **Task 008** — extend UTF-8 / Estonian character
  coverage (largely covered already).
- **Task 014** — DSL loader scale optimization (parallel
  loader / lazy Handlebars validation) — needed for
  operators who harvest the full RIA catalog via
  `scripts/harvest-xtee-wsdls.sh`.
- **First tag push**: `v0.1.0-rc.2` (already exists
  locally at `9434245`).

## Open backlog

| Task | Location | Status |
|---|---|---|
| **Epic — X-Road protocol compliance** | [`tasks/backlog/epic-xroad-protocol-compliance/`](./tasks/backlog/epic-xroad-protocol-compliance/) | 1 open task (004) |
| **Epic — Testing infrastructure** | [`tasks/backlog/epic-testing-infrastructure/`](./tasks/backlog/epic-testing-infrastructure/) | 2 open tasks (007, 008) |
| **Epic — Operational hardening** | [`tasks/backlog/epic-operational-hardening/`](./tasks/backlog/epic-operational-hardening/) | Empty (011 landed) |
| **Epic — Developer experience** | [`tasks/backlog/epic-developer-experience/`](./tasks/backlog/epic-developer-experience/) | 1 open task (014) |

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
| **Domain design — what XTR-on-Rust must do (v0.1.0-rc.2 MVP scope + roadmap)** | [`docs/DESIGN.md`](./docs/DESIGN.md) |
| Every generic rule this project follows | [`STANDARDS.md`](./STANDARDS.md) |
| CI security gate | [`.github/workflows/security.yml`](./.github/workflows/security.yml), [`deny.toml`](./deny.toml), [`.cargo/audit.toml`](./.cargo/audit.toml) |
| Publish workflow | [`.github/workflows/publish.yml`](./.github/workflows/publish.yml) |
| Private security disclosure | [`SECURITY.md`](./SECURITY.md) |
| Full change history | [`CHANGELOG.md`](./CHANGELOG.md) |
