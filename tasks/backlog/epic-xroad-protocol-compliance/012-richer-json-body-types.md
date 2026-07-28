# 012 — Richer JSON body types in xml_to_json translation

## Filed

2026-07-26 — surfaced during task 002 MVP live smoke test.
Filed under epic `xroad-protocol-compliance`.

## Severity

**Low**. Not a correctness bug — every response *is* valid JSON
today. But everything is a string, which forces every REST caller
to re-parse fields they know are numeric or boolean.

## Motivation

`translate::xml_to_json::translate_soap` currently converts every
XML text node to `Value::String`. So a response containing:

```xml
<capital>2500</capital>
<active>true</active>
<founded>2005-06-14</founded>
```

comes out as:

```json
{ "capital": "2500", "active": "true", "founded": "2005-06-14" }
```

Consumers need `parseInt`, `parseBool`, `Date.parse` on the far
side — friction against the "REST-native" value XTR is supposed
to provide.

## Fix

Extend `translate_soap` (or add a small `coerce_value` pass) to
recognise a small set of unambiguous shapes:

- Pure-digit string (optional leading `-`, no leading zeros unless
  the whole value is `0`) → `Value::Number`. Preserve very large
  values as `String` if `i64` / `u64` overflows — never lose data.
- `"true"` / `"false"` (case-insensitive?  probably case-sensitive
  is safer — X-Road payloads are typically lower-case) →
  `Value::Bool`.
- Empty element (`<x/>` or `<x></x>`) → `Value::Null` (already
  done today, keep).
- Everything else stays as `Value::String`. Dates, decimals with
  locale-specific separators, opaque IDs — leave them alone.

Do NOT try to coerce decimals (`"3.14"` → 3.14) — floating-point
representation loss is a classic footgun for financial data
(e.g. `capital: "3.10"` becoming `3.1`). Callers who want a
number from a decimal string can opt in on their side.

Do NOT infer arrays from a single-element pattern; the existing
"multiple siblings → array" rule already handles the sensible
case. Introducing schema-driven single-element arrays is a
separate v0.3+ discussion.

## Acceptance

- Unit tests for each coercion rule: `"42"` → number,
  `"true"` → bool, `""` → null, `"70006317"` → number (fits i64),
  `"99999999999999999999"` → string (overflow), `"3.14"` → string
  (no float coercion), `"007"` → string (leading zero preserved),
  `"01"` → string (leading zero preserved).
- Round-trip test: an Ariregister-shaped fixture is translated
  and every expected numeric field is a `Value::Number` in the
  output.
- Existing tests continue to pass — string-shaped fields the
  tests already assert against stay strings.

## Estimated effort

Half a day. The coercion logic is trivial; the value is in
covering the surprising corner cases (leading zeros, huge
numbers, decimals) so future contributors don't relax the rules
without noticing what breaks.

## Dependencies

None. Landing this after task 008 (UTF-8 round-trip test) means
its fixture set will already include realistic Estonian payloads
we can extend with numeric-field assertions.

## Non-goals

- Schema-driven typing (would require the DSL to declare response
  shapes; violates the "DSL is transport, not schema" contract
  established in DESIGN.md §2.5).
- Automatic date parsing (locale-dependent, silent-corruption
  prone; strings are safer).
