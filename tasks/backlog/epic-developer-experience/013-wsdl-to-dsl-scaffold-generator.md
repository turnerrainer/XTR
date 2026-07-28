# 013 — WSDL-to-DSL scaffold generator (`xtr scaffold`)

## Filed

2026-07-28 — surfaced during the "Adding a new service" docs
walkthrough. First actionable in the new
`epic-developer-experience`.

## Severity

**Medium**. Not a correctness issue; it's the biggest single
friction point when adding real services. Every new DSL today
requires the author to hand-type a SOAP envelope with correct
namespaces, correct element ordering, and the exact `<param>`
names the target expects — while cross-referencing a WSDL in
another window.

## Motivation

The current authoring loop, documented in
`book/src/dsl/adding-a-service.md`:

  1. Find the target's WSDL.
  2. Read it to figure out the operation name, namespaces, and
     required fields.
  3. Hand-write a `.yml` DSL with a Handlebars envelope that
     matches.
  4. Restart XTR, hit `curl`, iterate on parse errors and
     `Client.MissingParam` faults.

Steps 2-3 are pure toil. WSDLs are machine-parseable and the
target field names + types are literally in there. A one-shot CLI
that ingests a WSDL and emits a candidate DSL takes minutes to
implement per operation but saves hours per service integration.

## Fix

New CLI binary or subcommand: `xtr scaffold`.

Two invocation modes:

**Direct WSDL** (public SOAP endpoints like Ariregister):

```bash
xtr scaffold \
  --wsdl https://ariregxmlv6.rik.ee/?wsdl \
  --operation ettevottegaSeotudIsikud_v1 \
  > DSL/samples/ar/ettevottegaSeotudIsikud_v1.yml
```

**X-Road subsystem** (needs an SS to reach the target):

```bash
xtr scaffold \
  --xroad-target GOV/70000001/some-subsystem \
  --service-code someMethod \
  --config xtr.yaml \
  > DSL/samples/some-group/someMethod.yml
```

Output: a candidate DSL file with:

- `params:` populated from the operation's input message parts.
  Every leaf field in the request type becomes a param, in the
  order the WSDL declares them.
- `service:` set to the WSDL's `<soap:address location=…/>` for
  the direct case; omitted for the X-Road case.
- `method: POST` (only choice supported for scaffolding — see
  memory: XTR is POST-only by design).
- `envelope:` a full Handlebars template with the correct
  namespaces, operation element, and one `{{param}}` placeholder
  per input field.

For X-Road, the envelope also includes the standard header block
with `{{{generate.client}}}`, `{{generate.uuid}}`,
`{{generate.protocol_version}}`, and the target's
`<xroad:service>` identity — matching the pattern shown in
`book/src/dsl/adding-a-service.md` §"X-Road envelope pattern".

## Acceptance

- CLI runs standalone (no XTR server needed for direct-WSDL
  mode; the X-Road mode reuses the SS executor).
- Round-trip test: scaffold the 4 shipped Ariregister DSLs
  from the public WSDL; the generated files must be
  byte-equivalent (modulo whitespace + comment differences)
  to what's checked in. Any drift proves the generator is
  producing a real DSL, not just a pretty guess.
- Handles WSDL edge cases without panicking:
  - `xsd:choice` inside a message → emit both alternatives
    as a comment, pick one
  - Optional (`minOccurs=0`) fields → include but comment as
    optional
  - `xsd:complexType` nested field → recurse
  - Imported schemas (`xsd:import` from separate URL) →
    follow at most one level, then bail with a clear error
- Never silently omits a required field. If the generator
  can't confidently emit an envelope, it emits a partial
  scaffold with `# TODO: figure out X` comments — the author
  finishes the last mile, but scaffolding did the tedious part.
- Documented in `book/src/dsl/adding-a-service.md` as the
  preferred path (the hand-write path stays for reference).

## Estimated effort

- WSDL parser (or dependency on one — check `quick-xml` +
  `wsdl` crate ecosystem, or roll a minimal parser for just
  the subset SOAP 1.1 uses): 1 day
- Envelope templating + auto-context injection: half a day
- CLI plumbing (`clap` subcommand, output file handling): half
  a day
- Round-trip tests against shipped Ariregister DSLs: half a day
- Docs update: quarter day

Total: 2.5 days of focused work.

## Dependencies

- None hard. Reuses the existing `SsExecutor` for the X-Road
  mode; direct-WSDL mode is a fresh code path.
- Landing this after task 007 (mock SS harness) would let the
  X-Road round-trip test run in CI. Otherwise it needs a
  real SS reachable from the developer's machine.

## Non-goals

- Complete WSDL 1.1 coverage. Focus on the subset X-Road /
  Ariregister-style services actually use. Bail with a clear
  error on unsupported constructs; don't paper over.
- WSDL 2.0. Nobody in the X-Road ecosystem uses it.
- Auto-updating existing DSLs when a WSDL changes. That's a
  different feature — this one is one-shot scaffold only.
- Response-shape parsing / codegen. XTR's response is
  runtime-shaped from whatever the upstream returns; we don't
  need static types for it.
- REST client codegen from the resulting XTR endpoint. That's
  what the `/api` OpenAPI spec is for — client codegen is a
  standard OpenAPI-tooling problem, not our concern.
