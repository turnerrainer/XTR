# 011 — Request + response size caps

## Filed

2026-07-26 — surfaced during task 002 MVP review. Filed under
new epic `operational-hardening`.

## Landed

2026-07-28 — commit `b3b56ad` (combined with task 010). New
`Limits` config section with defaults (`max_request_bytes` 1 MiB,
`max_response_bytes` 16 MiB, `request_timeout_secs` 30). Inbound
cap enforced via a byte-exact check in the router handler +
`DefaultBodyLimit` transport backstop, producing structured 413
`XtrError::RequestTooLarge { limit }`. Outbound cap enforced via
a new `read_bounded` streaming reader shared between plain and
Security Server executors, producing structured 502
`XtrError::UpstreamBodyTooLarge { limit }` and tearing down the
connection immediately. Timeout hoisted from hardcoded 30s to
`Limits.request_timeout_secs`. 2 new integration tests
(oversized inbound + oversized upstream). Book note deferred to
a later docs sweep.

## Severity

**Medium**. Not exploitable today under a trusted Security Server,
but the moment XTR is fronted by any untrusted network path (or
the upstream itself misbehaves), an unbounded body is a memory
DoS. Ruuter-on-Rust fixed this early; XTR should not regress.

## Motivation

Two ceilings are missing:

1. **Inbound REST body cap.** `axum::extract::Json` will parse
   arbitrarily large bodies. A caller (or an attacker who reaches
   the REST surface) can send a multi-GiB JSON blob; axum will
   allocate and parse it before XTR ever gets to the DSL layer.
2. **Outbound response cap.** `reqwest`'s `resp.text().await` on
   an untrusted upstream will buffer the entire response into
   memory. A misbehaving Security Server (or an X-Road service
   that returns an accidentally huge document) will pin arbitrary
   memory per in-flight request.

Both were solved in Ruuter-on-Rust via `Limited<Body>` on the
inbound side and `.take(N)` on the outbound side; the same
mechanics apply here.

## Fix

Add to `AppConfig`:

```yaml
limits:
  max_request_bytes: 1048576         # 1 MiB inbound REST body
  max_response_bytes: 16777216       # 16 MiB upstream response
  request_timeout_secs: 30           # already effectively 30s via reqwest
                                     # — hoist to config
```

Enforce:

- **Inbound**: `axum::extract::DefaultBodyLimit::max(cfg.limits.max_request_bytes)`
  wired at the router level. Exceeded → 413 Payload Too Large with
  structured JSON error `{ "error": "request_too_large", "limit": N }`.
- **Outbound**: replace `resp.text().await` in both
  `src/executor/plain.rs` and `src/executor/ss.rs` with a bounded
  read that streams up to `max_response_bytes` and errors past
  that point. New error variant
  `XtrError::UpstreamBodyTooLarge { limit }` → 502.
- **Timeout**: keep the reqwest builder's `.timeout()` but read
  the value from config so operators can adjust for slow X-Road
  services.

## Acceptance

- Integration test: POST with a body larger than the configured
  cap returns 413 + the structured error. Under the cap succeeds.
- Integration test: mock upstream that streams more than
  `max_response_bytes` produces 502 + the structured error,
  with the connection torn down (no memory blowup).
- Timeout: mock upstream that never responds is torn down at
  `request_timeout_secs`; 504 with structured error.
- Documented in `book/src/ops/` with the reasoning for each
  default and how to raise them safely.

## Estimated effort

One day. Wiring the limits is small; the value is in the three
integration tests, which need synthetic slow / large / hanging
upstreams.

## Dependencies

- Task 007 (mock X-Road Security Server fixtures) would provide the
  slow/hanging upstream harness. Can also be done with an
  ad-hoc axum test server if 007 slips.

## Non-goals

- Per-service caps in the DSL. That's a v0.3 concern; the MVP
  hardening layer uses a single global cap.
- Rate limiting / concurrency caps. Deserves its own task in
  this epic when there's a concrete deployment target driving
  the policy.
