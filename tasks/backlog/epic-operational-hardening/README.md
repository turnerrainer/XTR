# Epic — Operational hardening

**What**: everything that keeps XTR-on-Rust safe and predictable
under adversarial or unexpected input in production. Size caps,
timeouts, rate limits, resource ceilings — the "defense in depth"
layer that sits above wire-level correctness.

**Why**: task 002 delivered a functional proxy. That proxy today
will happily forward a 4 GiB body to the upstream and echo a 4 GiB
response back to the caller, given enough memory. It has no
per-request budgets, no upstream body-size caps, no protection
against a slow-loris upstream. None of that matters for the MVP
demo; all of it matters the moment XTR runs in front of a
Security Server that services real requests.

**Closes when**:

- Configurable request-size, response-size, and timeout caps exist
  and are enforced on every request path.
- Overrun responses are structured errors, not panics or truncated
  bodies.
- All caps are documented in `book/src/ops/`.
- Rate-limit / concurrency policy exists at least as an ADR
  (implementation may land in a follow-up epic).

**Open tasks**: see sibling `NNN-*.md` files.

**Related**: DESIGN.md §8.7 (Configuration), STANDARDS.md §7
(the Ruuter-inherited "reject early" discipline).
