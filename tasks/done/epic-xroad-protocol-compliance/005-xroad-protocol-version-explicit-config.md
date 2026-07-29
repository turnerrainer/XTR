# 005 — Explicit X-Road protocol version in config (stop hardcoding `4.0`)

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`xroad-protocol-compliance`.

## Landed

2026-07-28 — commit `5ca6ad6`. Added `xroad_protocol_version` to
AppConfig (default: "4.0"), exposed as
`{{generate.protocol_version}}` in the Handlebars auto-context.
Both shipped X-Road samples (listMethods, allowedMethods)
migrated to reference the auto-context variable. 2 new unit
tests (default + override).

## Severity

**Low** — matters only when X-Road pushes a new protocol version.
Zero-cost to fix now; painful to fix later when the sample DSLs
have drifted.

## Motivation

The JVM XTR's shipped DSL samples hardcode:

```xml
<xroad:protocolVersion>4.0</xroad:protocolVersion>
```

inside each envelope. That's the current SOAP-based X-Road wire
protocol version. **When it changes** (X-Road v7 is REST-based
and a different beast entirely; X-Road v6 SOAP moves versions
occasionally), every DSL file that hardcodes `4.0` becomes
subtly wrong — the Security Server may accept it under a compat shim, or may
reject it, depending on the Security Server version.

Not something to fix in each DSL. Something to inject.

## Fix

1. Add config field:
   ```yaml
   xroad:
     protocol_version: "4.0"   # default; can be overridden by
                               # ops per deployment
   ```

2. Add auto-context Handlebars variable
   `{{generate.protocol_version}}` alongside `generate.uuid`,
   `generate.client`, `generate.instance`.

3. Migrate shipped DSL samples to use it:
   ```diff
   -<xroad:protocolVersion>4.0</xroad:protocolVersion>
   +<xroad:protocolVersion>{{generate.protocol_version}}</xroad:protocolVersion>
   ```

4. Document in `book/src/dsl/handlebars-context.md` (part of
   task 002's docs deliverable, but this task's landing might
   need to touch it).

## Acceptance

- Config field parses and defaults to `"4.0"`.
- Auto-context variable available in templates.
- Shipped DSL samples migrated + tests still pass.
- Docs updated.

## Estimated effort

Half a day. Mostly search-and-replace.

## Dependencies

- Task 002 (MVP) must have landed the Handlebars auto-context
  mechanism first. This task extends it, not creates it.

## Non-goals

- Any support for X-Road v7 (REST-based). That's a wholly
  different protocol; if we ever need it, it's a new service, not
  a config toggle on XTR.
- Per-DSL overrides. Config-global only. If a specific service
  needs a different version, the DSL author can hardcode inside
  that one envelope — the auto-context is just the default.
