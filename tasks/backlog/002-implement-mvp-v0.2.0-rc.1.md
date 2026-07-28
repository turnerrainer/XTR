# 002 — Implement XTR-on-Rust MVP (v0.2.0-rc.1)

## Filed

2026-07-28 — follows task 001. Precedes the first tag push.

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
