# 010 — SOAP Fault detection in upstream responses

## Filed

2026-07-26 — surfaced during task 002 MVP live smoke test against
Ariregister. Filed under epic `xroad-protocol-compliance`.

## Landed

2026-07-28 — commit `b3b56ad` (combined with task 011).
`translate_soap` now detects SOAP 1.1 and 1.2 Fault bodies and
maps them to `XtrError::UpstreamSoapFault { code, string, detail }`
→ HTTP 502 with a structured JSON error payload carrying the
fault code, message and detail body as top-level fields. 4 new
unit tests + 1 integration test cover both SOAP versions, the
detail extraction, and the regression case where an ordinary
element named "not_a_fault" must NOT trip the path.

## Severity

**Medium**. Correctness bug in disguise: a `<soap:Fault>` body
comes back with HTTP 200 (SOAP wraps errors in the envelope, not
in the transport status), so today XTR happily translates the
fault into JSON and returns it as if it were a successful business
response. The caller sees a 200 with `{ body: { Fault: {...} } }`
and has no framework signal that the request failed.

## Motivation

Executor code today (`src/executor/plain.rs`, `src/executor/ss.rs`)
maps *transport* failure to `XtrError::UpstreamHttpError`:

```rust
if !status.is_success() {
    return Err(XtrError::UpstreamHttpError { status, body: ... });
}
```

But SOAP Faults ride on HTTP 200. So the request:

```
POST /ar/lihtandmed_v3
```

against an X-Road service that returns a fault (missing param,
downstream service unavailable, authorization refused) currently
returns HTTP 200 to the REST caller with the fault embedded. That
is misleading and forces every consumer to hand-check the JSON
body shape for a `Fault` node.

## Fix

In `src/translate/xml_to_json.rs` (or a dedicated
`src/translate/fault.rs`):

1. After parsing the SOAP envelope, inspect the `Body` for a
   `Fault` child (SOAP 1.1: `soap:Fault`, SOAP 1.2:
   `soapenv:Fault` — match on local name, ignore prefix).
2. If present, do NOT translate to JSON as a normal body. Instead
   surface via a new error variant:
   `XtrError::UpstreamSoapFault { code, string, detail }`
   mapped to HTTP 502 with a structured JSON error payload:
   ```json
   { "error": "upstream_soap_fault",
     "code": "Client.MissingParam",
     "message": "Required parameter reg_code not supplied",
     "detail": { ... optional actor/detail payload ... } }
   ```
3. Log the fault at WARN with the outbound service name and DSL
   group, so operators can correlate.
4. Preserve the raw envelope in the log (truncated) for post-mortem.

## Acceptance

- Unit test: `translate::xml_to_json` on a fixture SOAP envelope
  with `<Fault>` returns `Err(UpstreamSoapFault { ... })`, not
  a successful body/headers pair.
- Integration test: mock upstream returns HTTP 200 with a SOAP
  Fault body; the XTR endpoint responds 502 with the structured
  JSON error above.
- Handles both SOAP 1.1 (`faultcode`/`faultstring`) and
  SOAP 1.2 (`Code/Value`/`Reason/Text`) shapes.

## Estimated effort

Half a day. The Fault detection is a small tree-walk; the
plumbing to the new error variant + response mapping is trivial.

## Dependencies

None hard. Fits well alongside task 004 (response requestHash
verification), which also does post-response envelope inspection.
