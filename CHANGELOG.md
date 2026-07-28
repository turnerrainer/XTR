# Changelog

All notable changes to XTR-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Task 002 MVP (v0.2.0-rc.1 candidate)

Working REST → SOAP → X-Road proxy per DESIGN.md §8. Implements
the module tree, HTTP surface, DSL loader, Handlebars expansion,
executor (plain + mTLS), XML → JSON translation, auto-generated
OpenAPI, and integration tests. 12 of the 17 JVM XTR bugs from
DESIGN.md §7 fixed:

  #1  subsystem_code (correctly spelled)
  #2  no @Value on statics — instance-field config
  #3  Handlebars: single-pass render with merged context
  #5  <xroad:client> element built correctly (no literal %s)
  #6  system trust store (no trust-all X509TrustManager)
  #7  response exposes both {body, headers}
  #8  route pattern is /:group/:service (not wildcard)
  #9  structured error responses ({error, message} + proper status)
  #10 /health endpoint
  #13 port 8080 everywhere (no 9010/9020/8080 confusion)
  #14 OpenAPI param type "string" (not "String")
  #15 no `Towarsd` typo
  #16 keystore password from env var, never a default

Modules added (src/*)
  * config/        — AppConfig with load_or_default (--config /
                     XTR_CONFIG / ./xtr.yaml search path)
  * dsl/           — XRoadTemplate, ServiceMap, loader::load_all,
                     handlebars::expand (unified single-pass render)
  * executor/      — PlainExecutor (system trust), SsExecutor
                     (mTLS via PKCS12 identity), Executor::dispatch
  * translate/     — xml_to_json::translate_soap emits
                     {body, headers} with namespaces preserved,
                     attributes as @-keys, repeats as arrays
  * router/        — axum routes + AppState wiring
  * openapi.rs     — build_spec walks ServiceMap, emits stable
                     OpenAPI 3.1 output
  * error.rs       — XtrError with IntoResponse
  * main.rs        — tokio + config load + assemble + serve

Tests (29 pass, 0 fail, 0 ignored)
  * 5 loader (walk, missing path, extensions, non-YAML skip,
    parse error)
  * 6 handlebars (allow-list filter, drop non-allowlist, auto
    context, generate.client shape, generate.uuid validity,
    single-pass regression guard)
  * 8 xml_to_json (body/headers extraction with namespace
    prefixes, UTF-8 Estonian chars, XML entity refs, repeat
    → array, attributes → @-keys, empty → null, malformed
    error, namespaced element names)
  * 5 openapi (empty map, one service, "string" type regression,
    requestBody.required toggle, response schema shape)
  * 5 integration (health, /api lists loaded services,
    end-to-end with mock upstream capturing outbound
    Content-Type + body, unknown-service 404, params filter)

DSL samples
  * DSL/samples/ar/{lihtandmed_v3, detailandmed_v2,
    ettevottegaSeotudIsikud_v1, tegelikudKasusaajad_v2}.yml
  * DSL/samples/xroad/{listMethods, allowedMethods}.yml

  Imported verbatim from buerokratt/XTR. Live smoke test loads
  all six into GET /api as OpenAPI operations.

Docs
  * book/src/dsl/format.md — new. DSL format, params allow-list,
    service field semantics, Handlebars auto-context, response
    shape, end-to-end example.
  * book/src/getting-started/run-locally.md — refreshed with
    real /health + /api output; shipped-sample invocation
    recipe.
  * book/src/getting-started/automated-tests.md — baseline
    updated to 29/0/0.
  * book/src/SUMMARY.md — new DSL section.

### Added — task epic system + follow-ups (earlier this cycle)

- **`docs/DESIGN.md`** — the domain design derived from a direct
  read of the original [buerokratt/XTR](https://github.com/buerokratt/XTR).
  Documents the JVM XTR's public surface, DSL format, config,
  request lifecycle, and 17 known bugs. Defines the XTR-on-Rust
  MVP scope (`v0.2.0-rc.1`), correctness fixes applied, non-goals,
  crate layout, roadmap to v1.0. Now includes **§2.7 X-Road
  protocol context** — the domain gotchas beyond mechanical
  translation, each cross-linked to a follow-up task.
- **`tasks/backlog/002-implement-mvp-v0.2.0-rc.1.md`** — next
  task on the roadmap: implement DESIGN.md §8 (the MVP slice).
- **Task epic system** in `tasks/backlog/epic-*/`. Three epics
  filed after the task 001 review, each with its own README:
  - `epic-xroad-protocol-compliance/` — 3 open tasks (003, 004,
    005: Content-Type, response requestHash verification,
    explicit protocol version in config).
  - `epic-operator-onboarding/` — 1 open task (006: X-Road cert
    acquisition + keystore setup docs).
  - `epic-testing-infrastructure/` — 2 open tasks (007, 008:
    mock X-Road Security Server for CI, UTF-8 / Estonian
    charset round-trip test).
- **Empty `main` branch** — orphan commit with a README
  redirecting to `dev`. Reserved for the future `v1.0.0`.

### Changed

- `HANDOFF.md` — roadmap section rewritten. Task 001 marked
  done; task 002 up next. New "Open backlog" table listing
  top-level tasks + epics.
- `README.md` Status section now surfaces the domain design.
- `book/src/introduction.md` first paragraph points at
  `docs/DESIGN.md`.
- **`STANDARDS.md` §13 extended** — task tracking now allows
  optional epic subdirectories (`tasks/backlog/epic-<slug>/`
  mirrored to `done/` on completion). New "Linking rule"
  clause: every commit must reference at least one task file.

### Task tracking

- Task 001 (deep-dive) moved from `backlog/` to `done/` with a
  Landed note.

## [0.1.0] - 2026-07-28

Initial scaffold. Standards-compliant repo skeleton — no shipped
domain functionality yet. Every rule from Ruuter-on-Rust's
`STANDARDS.md` applied from day one.

### Added

- Rust binary crate (`xtr-on-rust`) — placeholder `main.rs` that
  prints a scaffold notice and exits. MSRV pinned to 1.88.
- **CI workflows** (`.github/workflows/`):
  - `tests.yml` — matrix on `ubuntu-latest` + `ubuntu-24.04-arm`,
    `cargo fmt --check` + `cargo clippy --all-targets -- -D
    warnings` + `cargo test --release --no-fail-fast`.
  - `security.yml` — `cargo audit --deny warnings` + `cargo deny
    check all` on push/PR/daily cron.
  - `publish.yml` — multi-arch (`linux/amd64` + `linux/arm64`)
    Docker Hub + GHCR publish on release tag or
    `workflow_dispatch`. Cosign keyless signing, SPDX SBOM,
    in-toto provenance, Trivy vulnerability scan gates signing,
    smoke test both platforms. Supports SemVer pre-release tags
    with maturity-scoped moving tag (`:rc`, `:beta`, `:alpha`).
  - `docs.yml` — mdBook build + GitHub Pages deploy on push to
    `main`.
- **Supply-chain configs**:
  - `deny.toml` — Apache-2.0-compatible license allow-list, banned
    wildcards, crates.io-only sources.
  - `.cargo/audit.toml` — empty exceptions stub (mirror any
    entries here into `deny.toml`'s `[advisories].ignore`).
- **Hardened container**:
  - `Dockerfile` — multi-stage `rust:1.88-slim` →
    `debian:bookworm-slim`, non-root uid 1000, `tini` as PID 1.
  - `docker-compose.yml` — `read_only: true`, `cap_drop: [ALL]`,
    `no-new-privileges: true`, CPU + memory limits, `HEALTHCHECK`.
- **Documentation scaffold** (mdBook at `book/`):
  - Getting Started chapters: Prerequisites → Run it locally →
    Watch the automated tests pass → What to read next.
  - Ops chapter: Docker (with placeholder cosign verify recipe).
  - Light-on-white theme (`book/theme/custom.css`).
- **STANDARDS.md** — the reference document capturing every rule
  this project inherits. Reusable by any `<Product>-on-Rust`
  sibling.
- **SECURITY.md** — private disclosure recipe, response SLA,
  supported versions, CI supply-chain posture inventory.
- **HANDOFF.md** — entry point for the next contributor.
- **`tasks/backlog/001-domain-deep-dive-original-xtr.md`** —
  first task on the roadmap: analyse the original
  `buerokratt/XTR` and define XTR-on-Rust's domain surface.
