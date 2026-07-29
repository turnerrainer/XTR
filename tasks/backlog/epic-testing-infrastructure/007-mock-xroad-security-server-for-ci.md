# 007 — Mock X-Road Security Server for CI

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`testing-infrastructure`.

## Severity

**High for the epic; medium overall**. Without this, every
integration test either hits a real X-Road (impossible in CI)
or uses ad-hoc mocks that drift from real Security Server behaviour. This
task establishes the shared fixture layer that tasks 003, 004,
008 all lean on.

## Motivation

X-Road integration tests need a mock that:

1. Accepts SOAP POSTs on an HTTP endpoint the test controls.
2. Verifies specific outbound headers (Content-Type — task
   003, `<xroad:client>` — validity, `<xroad:service>` fields).
3. Returns pre-canned SOAP responses per requested service.
4. Optionally computes and returns valid `<xroad:requestHash>`
   values (task 004).
5. Optionally injects failures (5xx, malformed XML, wrong
   requestHash) for negative tests.

Real X-Road SSes do mTLS. For CI we skip that — the mock listens
on plain HTTP; `reqwest`'s test config disables the mTLS
requirement in test mode.

## Approach

Two candidate implementations:

1. **`wiremock-rs`** — well-known, easy to spin up, good API
   for header matching. Con: WireMock's semantics are
   optimised for HTTP APIs; SOAP body matching (XML shape
   assertions) is awkward.
2. **Custom `axum` fixture server** in `tests/support/mock_ss.rs`.
   Full control; can compute requestHash correctly; can be
   as strict or lenient as the specific test wants. Con: more
   code to maintain.

Recommendation: **Option 2 (custom axum)**. XTR's test needs
are specific enough that a bespoke fixture is cheaper to
maintain than fighting a general-purpose mock. ~200 LOC total.

## Deliverables

- `tests/support/mock_ss.rs` — spawns an axum server on
  `127.0.0.1:0`, hands the URL back to the test. Handles:
  - Storing an ordered log of received requests (URL, headers,
    body) for post-hoc assertions.
  - A per-test-installable response handler
    (`Fn(SoapRequest) -> SoapResponse`).
  - Convenience helpers for the common "return fixture at
    `<file>`" case.
- `tests/fixtures/` — one `.xml` file per shipped DSL sample,
  containing a canonical SOAP response.
- At least one `tests/it_smoke.rs` test using the mock,
  proving the setup works end-to-end.

## Acceptance

- `cargo test --no-fail-fast` passes with the mock in place.
- Adding a new integration test that captures headers +
  asserts on the body reads as ~10 lines of test code.
- CI's tests.yml still completes within its normal time budget
  (the mock adds negligible overhead — it's just an axum
  server in-process).

## Estimated effort

- Mock server: 1 day.
- Fixture set for shipped samples: half a day.
- First real test using it (smoke): half a day.
- Total: 2 days.

## Dependencies

- Task 002 (MVP) must have landed the crate structure — this
  task adds files under `tests/`.

## Non-goals

- Emulating full X-Road semantics (message signing, ACL
  enforcement, member registration). We're mocking the wire,
  not reimplementing X-Road.
- Fuzz-testing X-Road responses (separate task if we ever
  want it).
- Load testing. This is a functional mock, not a load
  generator.
