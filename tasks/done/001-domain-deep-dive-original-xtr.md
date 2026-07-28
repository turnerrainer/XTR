# 001 — Deep-dive on the original XTR (JVM) to define domain surface

## Filed

2026-07-28 — first task on the XTR-on-Rust roadmap. Precedes any
Rust implementation of domain code.

## Landed

2026-07-28 — [`docs/DESIGN.md`](../../docs/DESIGN.md) shipped.
Direct read of `github.com/buerokratt/XTR` at its then-current
`main`. Sections:

1. Executive summary
2. Anatomy of the JVM XTR (endpoints, DSL format, Handlebars
   context, request lifecycle, config, deployment)
3. Buerostack dependencies (external — Security Server + X-Road
   services; internal — none today)
4. Non-goals from reading the code
5. WORK-IN-PROGRESS pieces in the JVM (WSDL introspection —
   deferred)
6. Auto-generated OpenAPI shape
7. **17 known bugs and rough edges** documented so XTR-on-Rust
   doesn't reimplement them
8. **XTR-on-Rust MVP design (v0.2.0-rc.1)** — crate layout, HTTP
   surface, DSL format, Handlebars context (unified into one
   apply), response shape, executor (plain + mTLS), config,
   error handling, OpenAPI generation, observability, ports,
   container mount for the keystore
9. Roadmap for v0.3+ (hot-reload, richer JSON types, WSDL
   introspection, cross-DSL composition, v1.0 stability
   commitment)
10. Explicit v0.2.0-rc.1 non-goals
11. Cross-references
12. Two open questions for first-PR review

Cross-linked from:
- `HANDOFF.md` — replaces "read task 001" with "read `docs/DESIGN.md`"
- `README.md` — Status section + Documentation list
- `book/src/introduction.md` — first paragraph

## Notes for the next task

Task 002 should implement the v0.2.0-rc.1 MVP slice per
`docs/DESIGN.md` §8. Start with the crate layout in §8.1, wire up
the executor + Handlebars expansion + XML→JSON translation, add
integration tests using the shipped DSL samples with a mock
upstream (mockito or wiremock-rs).

## Severity

**Foundational** — no downstream domain work can start meaningfully
without this. Not urgent in the "production is down" sense.

## Motivation

The scaffold (v0.1.0) sets up the *how* — CI, publish, hardening,
docs, standards. It leaves the *what* undefined. Before we write a
line of Rust that actually does something, we need to know what XTR
does, how, and why.

## Scope of the deep-dive

Read [buerokratt/XTR](https://github.com/buerokratt/XTR) end-to-end
and produce a design document (`docs/DESIGN.md` in this repo) that
captures:

1. **Purpose** — what problem does XTR solve? In one paragraph.
2. **Public surface** — every HTTP endpoint, WebSocket channel, or
   message contract the JVM version exposes.
3. **Configuration** — every operator-facing config knob, with
   defaults and constraints.
4. **State** — what does XTR persist (if anything)? Where? With
   what durability guarantees?
5. **Dependencies on other services** — Buerostack sibling services
   (Ruuter, Resql, TIM, CronManager, DataMapper) that XTR reads from
   or writes to.
6. **Non-goals** — what XTR deliberately does NOT do, so the Rust
   reimplementation doesn't drift into scope creep.
7. **Known issues in the JVM version** — anything worth NOT
   reimplementing verbatim.
8. **Rust-implementation notes** — where the JVM idiom (Spring
   Beans, blocking IO) needs re-architecting for Rust (async,
   ownership).

## Deliverable

- `docs/DESIGN.md` in this repo (create the folder), roughly
  500–1500 lines depending on XTR's surface area.
- Cross-linked from `HANDOFF.md`, `README.md`, and
  `book/src/introduction.md`.

## Acceptance

- Any contributor reading `docs/DESIGN.md` cold can answer: what
  should the first-cut XTR-on-Rust ship? What can wait? What's
  explicitly out of scope?
- The document unblocks the next task: "implement minimal viable
  XTR slice in Rust".

## Non-goals for THIS task

- Writing any Rust domain code.
- Improving the JVM XTR.
- Documenting XTR-on-Rust's own future features (this task is about
  the *original*, as the source of truth for parity).

## Effort estimate

- Reading + note-taking: ~1 day if XTR is small (< 5k LOC), longer
  if bigger.
- Writing the design doc: another day.
- Total: ~2 focused days.
