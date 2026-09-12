# HTTP response contract

Every response XTR emits has a predictable shape. If you're
building a client, a monitor, or a pentest harness, this page
is the contract.

> **Upgrade note — additive-but-observable changes in audit-v2.**
>
> All three middleware layers described below are new relative
> to `0.3.0-rc`. A strict caller upgrading from `0.3.0-rc` may
> see:
>
> - **New response headers on every response.** Five
>   `content-security-policy` / `strict-transport-security` /
>   `x-frame-options` / `x-content-type-options` /
>   `referrer-policy` headers plus `traceparent` + `x-trace-id`.
>   Clients that key on the absence of these headers (rare)
>   would need an allow-list.
> - **Handler-level 504 after `limits.request_timeout_secs + 5s`.**
>   Previously a slow handler-side step (handlebars expansion,
>   XML translate) could hang until the client gave up. Health
>   checks that tolerated hangs may now see 504.
> - **One INFO access-log line per request** with method /
>   route / status / duration / trace_id. Log volume rises
>   accordingly; log-shippers may need a rate cap.
>
> None of these change the JSON error-body shape or the stable
> `error` codes CI pipelines pin to.

## Middleware pipeline

Requests flow through three router-level layers (audit-v2)
before reaching the handler:

```
Request
  ↓
access_log         (audit-v2 §1.2 + §1.6): mint / honour W3C trace-id, start timer
  ↓
security_headers   (audit-v2 §5.1): five default response headers
  ↓
TimeoutLayer       (audit-v2 §6.2): cap the whole handler at
                                    limits.request_timeout_secs + 5s → 504
  ↓
handler            (SOAP or REST dispatch)
  ↑
back through security_headers, access_log; response goes on the wire.
```

## Default response headers

Every response (health, /api, `/:group/:service` — including
error responses) carries:

| Header | Value |
|---|---|
| `content-security-policy` | `default-src 'none'; frame-ancestors 'none'` |
| `strict-transport-security` | `max-age=63072000; includeSubDomains; preload` |
| `x-frame-options` | `DENY` |
| `x-content-type-options` | `nosniff` |
| `referrer-policy` | `no-referrer` |
| `traceparent` | `00-<32-hex-trace>-<16-hex-span>-01` |
| `x-trace-id` | `<32-hex-trace>` (lowercased) |

The trace-id is reused from an inbound valid `traceparent`
header; otherwise a fresh v4 UUID is minted per request.

The middleware never overwrites a header already set upstream
(matters for the REST passthrough lane).

## Access log

One INFO line per request, structured:

```
INFO http_request_completed method=POST route="/:group/:service"
     status=200 duration_us=1234 trace_id=abc12345…
```

The `route` field is the matched pattern, not the raw URI —
log cardinality stays bounded regardless of caller-chosen path.
User-controlled values are Debug-formatted so CR/LF/ANSI in
the path renders as escape sequences (audit-v2 FN-LOG-1).

## Status codes emitted by XTR

| Status | Trigger | Error code |
|---|---|---|
| `200` | Successful SOAP/REST call | (n/a) |
| `400` | Malformed JSON body (audit-v2 FN3) | `invalid_json_body` |
| `404` | Unknown group/service | `template_not_found` |
| `404` | `/api` when `observability.expose_openapi=false` (audit-v2 F-XTR-1) | `not_found` |
| `405` | Method mismatch vs DSL contract | `method_not_allowed` |
| `413` | Request body over `limits.max_request_bytes` | `request_too_large` |
| `500` | Handlebars expansion / internal | `template_expansion_failed`, `internal_error`, `keystore_load_failed` |
| `502` | Upstream returned SOAP Fault, unrecognised HTTP error, XML parse error, or oversized body | `upstream_soap_fault`, `upstream_http_error`, `upstream_xml_parse_error`, `upstream_body_too_large` |
| `504` | Upstream timeout OR handler-level TimeoutLayer fired | `upstream_timeout` / (bare) |
| `599` | `XTR_OFFLINE=true` short-circuit (audit-v2 FN-LOG-3) | `xtr_offline` |

## JSON error body shape

Every non-2xx from XTR carries:

```json
{
  "error": "<stable_snake_case_code>",
  "message": "<free-form human-readable>"
}
```

Some variants add structured fields:

- `template_not_found` → `group`, `service` (each clipped to 256 chars)
- `method_not_allowed` → `method`, `group`, `service`
- `request_too_large` / `upstream_body_too_large` → `limit`
- `upstream_soap_fault` → `code`, `string`, and (if
  `expose_soap_fault_detail: true`) `detail`

Pin CI rules to `error` (the stable code), never to `message`.

## SOAP fault sanitisation (audit-v2 FN2)

When the upstream returns a SOAP Fault, XTR extracts `code` +
`string` + `detail`. Before serialising to the JSON body:

- Every C0 control char (`0x00`..`0x1F` except tab `0x09`)
  and `DEL` (`0x7F`) in `code` / `string` is replaced with
  `U+FFFD` (Unicode REPLACEMENT CHARACTER).
- `string` is capped at 200 chars (audit-v1 H3); the marker
  `… (truncated)` is appended only when trimming happened.
- `detail` is omitted unless `expose_soap_fault_detail: true`.

The sanitisation applies **on both paths** — the flag controls
detail visibility, not byte-transparent passthrough.

## Handler timeout (audit-v2 §6.2)

Every request has a hard ceiling of
`limits.request_timeout_secs + 5s` (grace on top of the
outbound reqwest cap). When the ceiling fires, the response is:

```
HTTP/1.1 504 Gateway Timeout
```

with no body — this is the `tower_http::timeout::TimeoutLayer`
default and matches the shape of the `UpstreamTimeout` variant.

## Test-safety mode

`XTR_OFFLINE=true` in the environment turns every outbound
call into an immediate HTTP 599:

```json
{ "error": "xtr_offline", "message": "outbound blocked: XTR_OFFLINE is set (test-safety mode)" }
```

No packets leave the container. Use for pentest / break-test
runs against XTR when the WSDL corpus points at real X-Road
services.

The doctor tool emits a WEAK `weak-offline-mode-active`
finding when the env var is set so an operator who
accidentally leaves it enabled sees it. Truthy values: `1`,
`true`, `yes`, `on` (case-insensitive).
