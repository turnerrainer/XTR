# 004 — Verify response `<xroad:requestHash>` against outbound request

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`xroad-protocol-compliance`.

## Severity

**Low** — no known exploit against XTR today (perimeter-trust
assumption), but this is defence-in-depth against a compromised
network path between XTR and the Security Server.

## Motivation

Every X-Road response includes a `<xroad:requestHash>` element in
its SOAP header — a hash of the outbound request's headers,
computed by the responding Security Server. Verifying that the response's hash
matches the request we actually sent proves the response is
genuinely a reply to *our* request and not a swap by a
compromised path segment.

The JVM XTR skips this. XTR-on-Rust's MVP also skips it — the
perimeter-trust assumption (XTR runs inside the Buerostack
network, next to its own trusted Security Server) makes it acceptable but not
best-practice.

## Fix

1. Compute the request hash on the way out of the executor. The
   algorithm is defined in the X-Road protocol spec (SHA-512
   over a canonicalised concatenation of the request headers —
   look up the current spec for the exact bytes to include).
2. Store the hash in the per-request context.
3. On response, parse out `<xroad:requestHash>` from the SOAP
   header. Compare to the stored hash.
4. On mismatch: fail the request with `XtrError::ResponseHashMismatch`
   → 502.
5. Add a config knob to bypass the check for weird environments:
   `xroad.response_hash_verify: bool` (default `true` once this
   task lands).

## Acceptance

- Passing X-Road response with correct hash → 200 as today.
- Same response with tampered `<xroad:requestHash>` → 502,
  clear log line naming the mismatch.
- Config knob toggles the check (integration test both ways).
- Docs updated in `book/src/ops/` explaining what the check
  protects against and when to disable it.

## Estimated effort

- Learning the X-Road spec's exact algorithm: half a day.
- Implementation + tests: half a day.
- Total: 1 day.

## Dependencies

- Task 007 (mock X-Road Security Server fixtures) — the mock needs to compute
  and return valid `<xroad:requestHash>` values, otherwise every
  integration test breaks the moment this check lands.

## Non-goals

- Verifying the Security Server's signature on the response (that's the Security Server's
  contract with us, not something XTR should re-verify).
- Any change to `<xroad:client>` or `<xroad:service>` header
  handling.
