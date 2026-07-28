# Failure modes

Every HTTP status XTR itself can emit, and what caused it. XTR
translates upstream failures into structured JSON errors following
the shape in
[src/error.rs](https://github.com/turnerrainer/XTR/blob/dev/src/error.rs).

## Response body shape

Every error response is JSON:

```json
{ "error": "<stable_code>", "message": "<human message>", ... }
```

Extra fields depend on the variant:

| Variant | Extra fields |
|---|---|
| `upstream_soap_fault` | `code` (SOAP faultcode), `string` (faultstring), `detail` (optional payload) |
| `request_too_large` / `upstream_body_too_large` | `limit` (byte cap that was exceeded) |

## Status table

| Status | `error` code | Cause |
|---|---|---|
| `404` | `template_not_found` | No DSL matched `/<group>/<service>`. |
| `413` | `request_too_large` | Inbound body exceeded `limits.max_request_bytes`. |
| `500` | `template_expansion_failed` | Handlebars rendering blew up at request time. (Startup validation catches most malformed templates before boot.) |
| `500` | `keystore_load_failed` | The `.p12` couldn't be read or parsed — usually wrong password, wrong file, or a legacy-cipher mismatch. Only fires on the mTLS path. |
| `500` | `internal_error` | Genuinely unexpected — check the log line. |
| `502` | `upstream_http_error` | Upstream returned a non-2xx status AND the body was not a parseable SOAP Fault. |
| `502` | `upstream_soap_fault` | Upstream returned a SOAP `<Fault>` — either on HTTP 200 or wrapped in HTTP 5xx. Structured error surfaces `code`/`string`/`detail`. |
| `502` | `upstream_xml_parse_error` | Upstream response wasn't valid XML we could parse. Includes the XXE-guard rejection for custom entities and the nesting-depth cap. |
| `502` | `upstream_body_too_large` | Upstream response exceeded `limits.max_response_bytes`. Connection is torn down. |
| `504` | `upstream_timeout` | Upstream didn't respond within `limits.request_timeout_secs`. |

## SOAP Fault detection

XTR recognises a SOAP `<Fault>` in either of two positions:

- **HTTP 200 with a Fault body** — the SOAP-spec-compliant shape.
- **HTTP 4xx / 5xx with a Fault body** — real-world providers
  (Ariregister among them) wrap faults in HTTP 500. XTR tries fault
  extraction before falling back to `upstream_http_error`.

Both SOAP 1.1 (`faultcode` / `faultstring`) and SOAP 1.2
(`Code/Value` + `Reason/Text`) shapes are handled, including
namespace-prefixed variants (`env:faultcode` etc).

## What XTR does NOT return

- `400` — malformed JSON in the request body is not an error; XTR
  treats non-object / unparseable bodies as "no params supplied"
  and lets the DSL's allow-list decide whether that's OK.
- `401` / `403` — XTR has no built-in auth. Auth is upstream of
  XTR (put it behind a reverse proxy or add auth in a Ruuter DSL
  in front).
- `429` — no rate limiting today. Track [operational-hardening
  epic](https://github.com/turnerrainer/XTR/tree/dev/tasks/backlog/epic-operational-hardening)
  for when this becomes relevant.

## See also

- [Configuration](./configuration.md) — for tuning the limits.
- [X-Road Security Server setup](./xroad-security-server.md) —
  common `keystore_load_failed` / mTLS pitfalls.
