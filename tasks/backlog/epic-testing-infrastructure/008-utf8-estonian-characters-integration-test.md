# 008 — UTF-8 / Estonian character round-trip integration test

## Filed

2026-07-28 — follow-up to task 001 review. Filed under epic
`testing-infrastructure`.

## Severity

**Low but important**. No known bug today — `quick-xml` handles
UTF-8 correctly when configured. But this is the kind of test
that catches silent regressions during a future refactor: someone
swaps XML libraries, defaults change, and person names
suddenly ship as `Peeter K??rp` instead of `Peeter Kärp`.

## Motivation

Real X-Road payloads routinely include:

- **Estonian characters**: `ä ö ü õ Š Ž` in person names, company
  names, addresses.
- **Special punctuation** in official document titles.
- **Right-to-left / accented characters** in international
  registrations (rarer, but possible).

XML → JSON translation is where these are most likely to break:

- Wrong content-type interpretation (Latin-1 assumed).
- Byte-order-mark (BOM) not stripped.
- Escape sequence handling for `&auml;` etc.
- JSON output not properly UTF-8-encoded.

The end-to-end path — SOAP body arrives → parsed → traversed →
JSON serialised → HTTP response body — has enough seams that only
an integration test can prove correctness.

## Deliverables

New test file: `tests/it_charset.rs`. Tests:

1. **UTF-8 body, direct-HTTPS path** — mock upstream returns a
   SOAP response containing `<name>Peeter Kärp</name>`. XTR
   receives, translates, returns JSON. Assert the returned
   JSON string has `name: "Peeter Kärp"` (exact UTF-8 bytes).
2. **UTF-8 body, Security Server path** — same, via the mock Security Server fixture
   (task 007).
3. **Numeric character reference** — response with
   `<name>Peeter K&#228;rp</name>` (which is `ä` as an XML
   entity). Assert the decoded JSON is still `Peeter Kärp`.
4. **Company name with diacritics** — response with
   `<name>Šarmi &amp; Žetooni OÜ</name>`. Assert the entity
   references decode and the JSON round-trips cleanly.
5. **Mixed content in element** — an element that contains both
   text and child elements (unusual for X-Road but not
   forbidden). Assert we don't lose the text.
6. **BOM at start of upstream response** — some services do
   this. Assert we strip it (or at least don't include it in
   the JSON output).

## Acceptance

- All 6 test cases pass.
- Any regression that breaks character handling fails the CI
  build with a diff that clearly shows the byte-level
  discrepancy.

## Estimated effort

- Test-case bodies: half a day.
- Fixture files (SOAP responses): half a day.
- Total: 1 day.

## Dependencies

- Task 007 (mock Security Server) for cases 2, 4, 5.
- Task 002 (MVP) has landed the XML→JSON translation code.

## Non-goals

- Testing every possible Unicode edge case (surrogate pairs,
  combining characters, RTL isolates). Estonian characters +
  common Central European additions are enough for our
  domain.
- Testing what happens with malformed UTF-8 in the response
  (that's an error-handling task, filed separately if the
  need arises).
