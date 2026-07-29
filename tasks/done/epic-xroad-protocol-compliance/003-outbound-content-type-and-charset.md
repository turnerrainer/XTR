# 003 — Content-Type + charset on outbound X-Road calls

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`xroad-protocol-compliance`.

## Landed

2026-07-28 — as part of task 002 Phase D (commit `752efe5`).
Both executors (`src/executor/plain.rs:36`,
`src/executor/ss.rs:55`) set
`content-type: text/xml; charset=utf-8` unconditionally on every
outbound request. DSL authors cannot override — as the task
scope specified. Integration test
`end_to_end_request_hits_upstream_and_translates_response`
(tests/it_end_to_end.rs) captures the outbound Content-Type on
the mock upstream and asserts it starts with `text/xml`; that
was already in place from Phase H. No follow-up needed except
the book note (deferred until the ops chapter is expanded).

## Severity

**Medium**. Some Security Servers accept requests without an
explicit charset and infer UTF-8; others don't. Getting this
wrong produces sporadic HTTP 415 responses that are hard to
attribute.

## Motivation

X-Road Security Servers expect the outbound SOAP envelope with:

```
Content-Type: text/xml; charset=utf-8
```

or the equivalent SOAP 1.2 media type:

```
Content-Type: application/soap+xml; charset=utf-8
```

The JVM XTR relies on Spring's default, which happens to set an
acceptable value most of the time. Rust `reqwest` sends no
Content-Type by default when you pass a string body — it'll
be empty. That would produce 400 / 415 from the Security Server immediately.

## Fix

In the executor step of the request path (DESIGN.md §8.6):

- **Direct HTTPS path** (`RequestExecutor::plain`): explicitly set
  `Content-Type: text/xml; charset=utf-8` on the outbound request
  regardless of the DSL. Some upstream services (public XML
  endpoints like Ariregister) may accept `application/xml` too,
  but `text/xml` is the safe default.
- **mTLS path via Security Server** (`RequestExecutor::ss`): same header.
- Emit the outbound Content-Type in the WARN/ERROR log line when
  the upstream returns 4xx, so misconfiguration is diagnosable.

Do NOT let DSL authors override Content-Type in the MVP —
providing a hook for that is a v0.3 concern if a real use case
appears.

## Acceptance

- Integration test that captures the outbound request headers on
  the mock Security Server (via task 007's mock server) and asserts
  `content-type: text/xml; charset=utf-8`.
- Same assertion for the direct-HTTPS path against a mock upstream.
- Documentation note in `book/src/ops/` explaining this is
  hardcoded and why.

## Estimated effort

Half a day including the test. Trivial code change; the value is
in the CI regression test.

## Dependencies

- Task 007 (mock X-Road Security Server fixtures) provides the wire-capture
  hook that makes the assertion cheap. If task 007 isn't ready,
  this task can still land using an ad-hoc `axum` capture server
  in the test.
