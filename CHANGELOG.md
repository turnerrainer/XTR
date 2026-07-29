# Changelog

All notable changes to XTR-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0-rc.2] - 2026-07-29

Second release candidate. Adds runtime WSDL folder-drop
(task 013), ships real Ariregister WSDL + 34 companion XSDs
under `wsdl/ar/` as the canonical source of truth (yields 33
auto-generated `/ar/*` endpoints on every boot), and renames
all "SS" abbreviations to "Security Server" throughout code,
config, DSLs, docs, and task files.

### Added — Task 013 WSDL folder-drop

- `wsdl_watch_dir` config field. At boot, XTR scans
  `<dir>/<group>/*.wsdl`, parses each, and generates
  `DSL/<group>/<operation>.yml` per `wsdl:operation`.
- SOAP-1.1 document/literal parser in `src/wsdl/` — supports
  inline anonymous complexTypes, named top-level complexTypes
  (with lazy resolution + cycle guard), `xsd:include` via a
  local-filesystem loader (offline discipline preserved),
  `xsd:import` skipped as framework, `xsd:annotation` skipped
  as documentation. Bail-out-on-unsupported for `xsd:choice`,
  WSDL 2.0, RPC/encoded, MIME attachments.
- Per-op lenient: unresolvable input elements log WARN and
  skip that operation; sibling operations still generate.
- Deterministic YAML output — same WSDL always produces
  byte-equal bytes.
- Generated DSLs carry a marker header. Hand-written DSLs
  (no marker) always win on collision with a WARN log.
- Optional `<wsdl>.meta.yaml` sidecar opts into X-Road
  envelope wrapping (member_class/member_code/subsystem_code
  → auto-generated `<xroad:*>` header block).
- Generator recognises the X-Road `TURVASERVER` placeholder
  in `<soap:address location=…/>` and omits `service:` so
  the executor routes via `security_server:` instead.

### Added — WSDL as source of truth

- `wsdl/ar/` — real Ariregister WSDL + 34 companion XSDs
  (~180 KB) vendored into the repo.
- `xtr.yaml` (new) — default config that ships with the
  repo. `docker compose up` / `cargo run` now boots with
  33 Ariregister endpoints live via WSDL ingestion.
- `.gitignore` — `/DSL/ar/*.yml` ignored (regenerated per
  boot from the WSDL). Hand-written DSLs (like `DSL/xroad/*`)
  stay tracked.
- Removed the 4 previously hand-written Ariregister sample
  DSLs (`lihtandmed_v3`, `detailandmed_v2`,
  `ettevottegaSeotudIsikud_v1`, `tegelikudKasusaajad_v2`) —
  now auto-generated with WSDL-native param names (Estonian
  `ariregistri_kood` instead of English `reg_code`).

### Changed — SS → Security Server

Renamed every "SS" abbreviation to "Security Server" across
code, configs, DSLs, book chapters, task files, comments,
and CHANGELOG entries. Rationale: the "SS" abbreviation
carries a well-known historical reputation that reads
unprofessional in a European government-infrastructure
context. See `feedback_never_abbreviate_security_server.md`.

- `SsExecutor` → `SecurityServerExecutor`
- `src/executor/ss.rs` → `src/executor/security_server.rs`
- `Executor.ss` field → `Executor.security_server`
- All prose in `book/`, `docs/`, `tasks/`, comments — same.
- SOAP protocol literals unchanged (`SOAP-ENV:Server`,
  `env:Server` etc are external error codes and must
  stay as-is).

### Docs

- `book/src/ops/wsdl-ingestion.md` — folder layout, marker
  semantics, override rules, X-Road sidecar convention,
  "no admin HTTP endpoint" rationale.
- `book/src/dsl/adding-a-service.md` — reframed as override
  fallback path; WSDL-drop is primary now.

Verified: 82/0/0 tests, fmt clean, clippy -D warnings
clean, mdbook + linkcheck build clean, live smoke boots
with 35 endpoints (33 auto-generated Ariregister + 2
hand-written X-Road samples).

## [0.2.0-rc.1] - 2026-07-28

First publishable release candidate. Working REST → SOAP →
X-Road proxy in Rust, live-verified against public Ariregister
endpoints. Everything below in this section is what ships in
this tag.

### Added — Post-MVP hardening sweep (2026-07-28)

Landed tasks 003, 005, 010, 011, 012, and a follow-up security
sweep in a single day. Test count 29 → 51 (0 fail, 0 ignored).

**Task 010 — SOAP Fault detection**. HTTP 200 + `<soap:Fault>`
now maps to a structured 502 `upstream_soap_fault` with
`code` / `string` / `detail` top-level fields, instead of silently
being translated as a successful response. Handles SOAP 1.1 and
1.2 including namespace-prefixed variants and `xml:lang`-tagged
Reason elements.

**Task 011 — Request/response size caps + timeout config**. New
`limits:` config section (`max_request_bytes` 1 MiB,
`max_response_bytes` 16 MiB, `request_timeout_secs` 30). Inbound
overflow → 413 `request_too_large`; upstream overflow → 502
`upstream_body_too_large` with the connection torn down
immediately. Outbound responses read chunk-by-chunk via a new
`read_bounded` helper — bounded memory per request.

**Task 012 — JSON type coercion**. Bare integer leaves become
`Value::Number`; `true`/`false` become `Value::Bool`. Deliberate
non-goals with enforcing tests: no float coercion (precision loss
on `"3.10"`), no leading-zero coercion (`"007"` stays string —
those are opaque IDs), no case-insensitive booleans, `i64`
overflow keeps raw string, attributed-leaf `#text` stays string.

**Task 005 — Explicit X-Road protocol version in config**. New
`xroad_protocol_version: "4.0"` config field exposed as
`{{generate.protocol_version}}` in the Handlebars auto-context.
The two shipped X-Road DSL samples (`listMethods`,
`allowedMethods`) migrated to the auto-context variable —
protocol-version changes now require a single config line update
instead of touching every DSL.

**Task 003 — Content-Type + charset on outbound calls**. Closed
as landed with task 002 Phase D — both executors already set
`text/xml; charset=utf-8`; existing integration test already
captured + asserted it. Marker added to `done/`.

### Security sweep

**quick-xml 0.36 → 0.41**. `cargo audit` flagged two
high-severity DoS advisories (RUSTSEC-2026-0194 quadratic on
duplicate attribute names, RUSTSEC-2026-0195 unbounded
namespace-declaration allocation) — both fixed in 0.41. Both
directly relevant since XTR parses untrusted upstream XML on
every request; size caps alone don't help against the quadratic
runtime.

**XXE guard**. quick-xml 0.41 introduced `Event::GeneralRef`
for entity references outside the XML-predefined set. Character
references (`&#nnn;`, `&#xhh;`) resolve to Unicode codepoints
via a new `decode_char_ref` helper. Custom entities
(`&nbsp;`, `&copy;`) are rejected with an explicit
`XmlParseError` mentioning XXE risk — accepting them would
require a DOCTYPE, which is the XXE attack surface.

**Nesting-depth cap (MAX_NESTING_DEPTH = 512)** on
`parse_children`. Prior state: unbounded recursion — a document
with hundreds of thousands of `<a><a><a>…` levels blew the
stack. Real envelopes rarely exceed 10 levels; cap gives ~50x
headroom.

**Regression coverage** added: Handlebars single-pass re-render
safety, malformed-body handling (7 shapes), percent-encoded-slash
path traversal, XML nesting cap, hex character ref, custom
entity XXE guard.

Final audit posture: `cargo audit` 0 advisories, `cargo deny
check` green on advisories/bans/licenses/sources.

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
  * executor/      — PlainExecutor (system trust), SecurityServerExecutor
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

[Unreleased]: https://github.com/turnerrainer/XTR/compare/v0.2.0-rc.2...HEAD
[0.2.0-rc.2]: https://github.com/turnerrainer/XTR/compare/v0.2.0-rc.1...v0.2.0-rc.2
[0.2.0-rc.1]: https://github.com/turnerrainer/XTR/compare/v0.1.0...v0.2.0-rc.1
[0.1.0]: https://github.com/turnerrainer/XTR/releases/tag/v0.1.0
