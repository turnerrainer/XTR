# 006 — X-Road certificate acquisition + keystore setup docs

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`operator-onboarding`.

## Severity

**Medium** — blocks first-time operator success. Every operator
hitting XTR-on-Rust for the first time will need this; not having
it turns a 30-minute setup into a multi-day support ticket.

## Motivation

The JVM XTR's docs assume the reader already knows how to obtain
X-Road certs and produce a PKCS12 keystore. That's true for
Estonian government IT teams — X-Road is table-stakes there —
but false for anyone else piloting XTR (including us, initially).

Two environments matter, each with different cert authorities:

- **`ee-test`** — X-Road test instance. Certs are self-service
  registration via RIA (Estonian Information System Authority).
  Free. Meant for development, integration testing, learning.
- **`ee-prod`** — production X-Road. Certs require a regulated
  registration process, an operational Security Server owned by
  your organisation, and typically a formal onboarding with RIA.
  Not something you spin up in an afternoon.

XTR-on-Rust needs docs covering both flows.

## Deliverables

New page: `book/src/ops/xroad-certificates.md`. Covers:

1. **What X-Road actually is** in two paragraphs — enough that
   an operator who's never heard of it gets the shape without
   reading the RIA documentation.
2. **`ee-test` cert acquisition** — step-by-step, with links to
   the RIA self-service portal and screenshots of the current
   flow. Should end with "you now have a `.p12` file".
3. **`ee-prod` cert acquisition** — brief overview of what the
   real process looks like, pointer to RIA docs, honest
   statement that this is not a self-service flow.
4. **Keystore layout** — where to mount the `.p12` file in the
   container, how the password is provided (env var, per
   STANDARDS.md — not baked into config), how to rotate.
5. **`security_server.url` per environment** — the well-known
   test SS endpoints, production endpoints depend on your
   organisation's own SS.
6. **First-run smoke test** — a `curl` recipe that exercises
   one of the sample DSLs (Ariregister is fine — it doesn't
   require full X-Road membership for the public XML feeds)
   plus one recipe that goes through the SS.
7. **Common failures** — `SSLHandshakeException` (wrong cert
   for wrong environment), `401 from SS` (subsystem not
   authorized for the service), `503 from upstream service`
   (real service is down; not our fault).

Cross-link from README, HANDOFF, `book/src/getting-started/`
prerequisites chapter.

## Acceptance

- `book/src/ops/xroad-certificates.md` exists and mdBook renders
  it cleanly.
- The Prerequisites chapter has a "for real X-Road use" section
  pointing at it.
- SECURITY.md's "What is out of scope" section acknowledges
  operator-side cert management as their responsibility, with a
  link.

## Estimated effort

- Research (RIA docs, current portal shape): half a day.
- Writing: half a day.
- Screenshots + review: half a day.
- Total: 1.5 days.

## Non-goals

- Automating cert acquisition. The `ee-test` self-service flow
  is a browser-based registration; not scriptable for XTR.
- Managing certs on the operator's behalf (Vault integration,
  cert rotation daemons — see SECURITY.md's "not our
  responsibility" list).
- Real-time monitoring of cert expiry (nice-to-have, not this
  task).
