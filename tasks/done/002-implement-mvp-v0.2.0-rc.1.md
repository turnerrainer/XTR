# 002 — Implement XTR-on-Rust MVP (v0.2.0-rc.1)

## Filed

2026-07-28 — follows task 001. Precedes the first tag push.

## Landed

2026-07-28 — 10 commits (phases A–J). Full working
REST → SOAP → X-Road proxy per DESIGN.md §8. Ready for the
`v0.2.0-rc.1` release cut (task 009).

Commits (all reference `tasks/backlog/002-…` per the linking rule):

| Phase | Commit | What |
|---|---|---|
| A | 025303d | Module scaffolding + all deps (axum, reqwest, handlebars, quick-xml, uuid, tracing, thiserror) |
| B | ea98661 | AppConfig + DSL loader (5 tests) |
| C | 9901225 | Handlebars expansion with unified auto+user context, single-pass render (6 tests) |
| D | 752efe5 | Executor — plain HTTPS + mTLS via PKCS12, no trust-all |
| E | 2c98a11 | SOAP XML → JSON translation (`quick-xml`), emits `{body, headers}` (8 tests) |
| F | ce4b99a | Router + XtrError → structured HTTP responses; `main.rs` wired |
| G | 55980d4 | OpenAPI 3.1 auto-generation from loaded DSLs (5 tests) |
| H | 76c8951 | Integration tests + shipped DSL samples imported from JVM XTR (5 tests) |
| I | ae3ad1a | Book DSL format chapter; HANDOFF + CHANGELOG refresh |
| J | (this)  | Move task to done; file task 009 (release prep) |

Bugs fixed
  Of DESIGN.md §7's 17 JVM XTR bugs, this MVP fixes 12:
  #1, #2, #3, #5, #6, #7, #8, #9, #10, #13, #14, #15, #16.

  The remaining 5 (#4, #11, #12, #17, and part of #7) are
  deferred to v0.3 or filed under
  `tasks/backlog/epic-*/` as dedicated follow-ups (see
  epic READMEs).

Verification (from Phase J final check)
  * cargo test: 29 passed / 0 failed / 0 ignored
    (24 unit across config/dsl/handlebars/translate/openapi +
    5 integration exercising the full HTTP surface with an
    in-process mock upstream)
  * cargo fmt --check clean
  * cargo clippy --all-targets -- -D warnings clean
  * mdbook build clean
  * Live smoke: server boots, loads 6 shipped DSLs, /health
    returns {"status":"ok"}, /api lists all 6 DSLs as OpenAPI
    operations

Follow-up

Task 009 filed for the release-prep + tagging work
(Cargo.toml bump to 0.2.0-rc.1, CHANGELOG version header,
docker-compose.yml image tag, README/HANDOFF pull-recipe
version). Same shape as Ruuter's release-prep PR.

## Scope

Implement the MVP slice defined in
[`docs/DESIGN.md`](../../docs/DESIGN.md) §8. This is the first
real domain PR — turns the scaffold into a working service.

## Deliverables

- **HTTP surface** (per DESIGN.md §8.2):
  - `POST /:group/:service` — invoke a mapped X-Road service
  - `GET /api` — auto-generated OpenAPI 3.1
  - `GET /health` — `{"status":"ok"}`
- **Crate layout** per DESIGN.md §8.1 (config, dsl, router,
  executor, translate, openapi, error modules)
- **DSL loader** — walks `config.dsl_path` recursively, builds
  `HashMap<(String, String), Arc<XRoadTemplate>>`
- **Handlebars expansion** — one apply, merged user + auto
  context. Auto context: `generate.uuid`, `generate.instance`,
  `generate.client`
- **Executor** — two backends (plain HTTPS, mTLS via PKCS12
  keystore to Security Server)
- **XML → JSON translation** — expose both `body` and `headers`
  in response (DESIGN.md §8.5)
- **Structured errors** — `XtrError` enum with `IntoResponse`
- **Traceparent propagation** — same as Ruuter
- **Integration tests** — cover the request path with mock
  upstream (mockito or wiremock-rs). Use shipped DSL samples
  from the original XTR repo (already committed at
  `/tmp/xtr-original/DSL/` — copy to `DSL/samples/` here)

## Correctness fixes to apply (from DESIGN.md §7)

Numbered from DESIGN.md §7:
- #1 — `subsystem_code` (correct spelling)
- #2 — no static-field `@Value` mistake; use instance fields
- #3 — Handlebars applied once with merged context
- #6 — use system trust store, not trust-all
- #7 — expose SOAP headers alongside body
- #8 — `/:group/:service` route pattern
- #9 — structured error responses
- #10 — `/health` endpoint present
- #13 — port 8080 everywhere
- #14 — OpenAPI param type `"string"` not `"String"`
- #15 — no typo'd method names

## Explicitly NOT in this task

Per DESIGN.md §10:
- Hot-reload of DSLs (v0.3)
- Rich JSON body types beyond `HashMap<String, String>` (v0.3)
- Per-DSL response extraction rules (v0.3)
- WSDL introspection (v0.4)
- Auth on XTR endpoints
- Rate limiting
- Response caching
- Cross-DSL composition

## Acceptance

- `cargo test --no-fail-fast` passes
- `cargo fmt --check` clean
- `cargo clippy --all-targets -- -D warnings` clean
- `cargo audit --deny warnings` + `cargo deny check all` clean
- `docker compose up -d --build` boots; `curl /health` returns
  `{"status":"ok"}`
- At least one shipped DSL sample can be exercised end-to-end
  against a mock upstream in the test suite
- `curl /api` returns a valid OpenAPI 3.1 document with the
  shipped samples as operations

## Effort estimate

- Scaffold-to-working-service: ~3 focused days
- Integration test suite: ~1 day
- Docs updates (book/src/dsl chapters describing DSL format,
  book/src/ops docker page, README pull recipes): ~1 day
- Total: ~1 working week

After acceptance, cut `v0.2.0-rc.1` following the release
flow in STANDARDS.md §10.
