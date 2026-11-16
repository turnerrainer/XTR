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
| `400` | Raw-XML: declared encoding not UTF-8 | `unsupported_encoding` |
| `400` | Raw-XML: prolog not well-formed (declaration not first, repeated, bad version, text outside root) | `malformed_xml` |
| `401` | `XTR_INTER_SERVICE_TOKEN` set and bearer missing/wrong on a gated route | `unauthorized` |
| `404` | Unknown group/service | `template_not_found` |
| `404` | `/api` when `observability.expose_openapi=false` (audit-v2 F-XTR-1) | `not_found` |
| `405` | Method mismatch vs DSL contract | `method_not_allowed` |
| `413` | Request body over `limits.max_request_bytes` | `request_too_large` |
| `422` | Raw-XML: message or request fails XSD validation when the direction is `enforce` | `schema_invalid` |
| `500` | Handlebars expansion / internal / XSD validator error (fail-closed) | `template_expansion_failed`, `internal_error`, `keystore_load_failed`, `validator_failed` |
| `502` | Upstream returned SOAP Fault, unrecognised HTTP error, XML parse error, or oversized body | `upstream_soap_fault`, `upstream_http_error`, `upstream_xml_parse_error`, `upstream_body_too_large` |
| `504` | Upstream timeout OR handler-level TimeoutLayer fired | `upstream_timeout` / (bare) |
| `508` | Raw-XML: `x-xtr-forwarded: 1` header on a request that would forward again | `loop_detected` |
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

## Schema-aware SOAP lanes

Details: [Schema-aware SOAP lanes](./soap-lanes.md). Both lanes pass
through the same middleware (security headers, access log, timeout).

### Inbound — `/soap-in/<group>/<name>`

Speaks SOAP 1.1 to SOAP clients; **every** error is a SOAP Fault, never
the JSON error body.

| Request | Status | Body |
|---|---|---|
| `POST`, operation with an output element | `200` | `text/xml; charset=utf-8` SOAP envelope; inbound `<Header>` children echoed |
| `POST`, one-way operation (no output) | `202` | empty |
| `POST`, malformed XML / `DOCTYPE` / not an Envelope / empty Body / unknown operation / `SOAPAction` mismatch | `500` | Fault `SOAP-ENV:Client` |
| `POST`, SOAP 1.2 envelope | `500` | Fault `SOAP-ENV:VersionMismatch` |
| `POST`, backend 4xx | `500` | Fault `SOAP-ENV:Client`, `faultstring` = backend `message`/`error` |
| `POST`, backend 5xx | `500` | Fault `SOAP-ENV:Server`, `faultstring` = `backend returned HTTP <status>` (backend message only with `expose_soap_fault_detail`) |
| `POST`, backend unreachable / timeout / no backend configured | `500` | Fault `SOAP-ENV:Server` |
| `POST`, backend `{"fault": {…}}` | `500` | Fault with the backend's `code` / `string` / `detail` |
| `POST`, body over `limits.max_request_bytes` | `413` | Fault `SOAP-ENV:Client` |
| `POST`, no such service | `404` | Fault `SOAP-ENV:Client` |
| `GET …?wsdl` | `200` | the WSDL, `soap:address` rewritten |
| `GET /soap-in/<group>/<file>.xsd` | `200` / `404` | a local XSD the WSDL includes, else 404 |

Fault shape (`faultstring` control characters → U+FFFD, max 400
chars) — captured from the [runnable example](./soap-lanes.md#try-it)
with the X-Road header removed:

```console
<?xml version="1.0" encoding="UTF-8"?>
<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/"><SOAP-ENV:Body><SOAP-ENV:Fault><faultcode>SOAP-ENV:Client</faultcode><faultstring>backend returned HTTP 403: X-Road-Client header is missing or has invalid format</faultstring></SOAP-ENV:Fault></SOAP-ENV:Body></SOAP-ENV:Envelope>
```

### Outbound — `/soap-out/<group>/<name>/<operation>`

JSON in, JSON out, regular [JSON error body](#json-error-body-shape).
Gated by `XTR_INTER_SERVICE_TOKEN` when set (`401`).

| Outcome | Status | Body |
|---|---|---|
| peer replied | `200` | `{"header": … \| null, "response": …}` |
| peer replied with an empty 2xx (one-way) | `202` | `{"header": null, "response": null}` |
| body not a JSON object | `400` | `invalid_json_body` |
| unknown service / operation / no `outbound:` | `404` | `template_not_found` |
| peer SOAP Fault | `502` | `upstream_soap_fault` (detail gated by `expose_soap_fault_detail`) |
| peer non-2xx without Fault | `502` | `upstream_http_error` |
| timeout | `504` | `upstream_timeout` |
| `XTR_OFFLINE` | `599` | `xtr_offline` |

## Raw-XML lanes

Details: [Raw-XML lanes](./xml-lanes.md). Same middleware stack
(security headers, access log, timeout) as the other lanes.
Peers exchange bare XML over HTTPS — no SOAP envelope — so the
inbound side does **not** emit SOAP Faults for its errors; it
emits the regular [JSON error body](#json-error-body-shape).

### Inbound — `/xml-in/<group>/<name>` (or the `inbound.path` alias)

Peers push XML; XTR decodes to JSON and POSTs to the configured
backend (or relays the bytes to a `forward:` target). The lane has
**no authentication** at the XTR layer — gate it at the ingress
(IP allowlist or mTLS).

XSD validation runs between the parser and the backend when
`validate.inbound` is set. See
[xml-lanes.md → Modes](./xml-lanes.md#modes) for the full state
machine; the status table below covers the response shape per
mode.

| Request | Status | Body |
|---|---|---|
| valid XML, backend 2xx | `202` (or `inbound.success_status`) | empty |
| valid XML, backend 4xx on `relay_statuses` (default 400/409/422) | relayed status | backend's body as `application/json`, cut at `limits.max_response_bytes` |
| valid XML, backend 4xx not on the list, or 5xx | `502` | `upstream_http_error` |
| valid XML, `forward:` configured, target 2xx/4xx | relayed status | empty on target over limit / unreadable |
| declared encoding not UTF-8 / utf8 | `400` | `unsupported_encoding` |
| prolog not well-formed, `DOCTYPE`, root element in a foreign namespace, body over the limit | `400` / `413` | `malformed_xml` / `request_too_large` |
| XML breaks the XSD, `validate.inbound: enforce` | `422` | `schema_invalid` with up to 10 `{path, rule, line, schemaFile, schemaLine}` violations (no message values) |
| XML breaks the XSD, `validate.inbound: report` | passthrough (backend decides) | violations logged, message still forwarded |
| XML breaks the XSD, `validate.inbound: annotate` | backend decides | backend receives `schemaValid: false` + `violations` + `rawBase64`; unreadable input routes to `<backend>/_invalid` |
| internal validator error (any mode) | `500` | `validator_failed` — XTR fails closed: no backend call, no forward, no outbound |
| `x-xtr-forwarded: 1` and the lane would forward again | `508` | `loop_detected` |
| `XTR_OFFLINE` | `599` | `xtr_offline` |

### Outbound — `/xml-out/<group>/<name>/<message>`

JSON in, peer's raw XML answer surfaced as a JSON envelope. Gated
by `XTR_INTER_SERVICE_TOKEN` when set (`401`). Per-message
timeout override via `messages.<M>.timeout_secs`.

| Outcome | Status | Body |
|---|---|---|
| peer replied | `200` | `{"status": <peer status>, "body": …, "contentType": …}` |
| body not a JSON object | `400` | `invalid_json_body` |
| unknown service / unknown message / no `outbound:` | `404` | `template_not_found` |
| serialised request breaks the XSD, `validate.outbound: enforce` | `422` | `schema_invalid` (no bytes leave XTR) |
| internal validator error (any mode) | `500` | `validator_failed` |
| peer refused / unreachable / empty / timeout | `502` / `504` | peer URL is not leaked |
| `XTR_OFFLINE` | `599` | `xtr_offline` |

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
