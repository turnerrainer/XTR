# Failure modes

Every HTTP status XTR emits for its own errors, with the stable
`error` code and cause. Upstream 4xx/5xx on the REST lane behave
differently — see "REST passthrough" below.

> Also see [HTTP response contract](./http-contract.md) for the
> middleware pipeline, default headers, and the per-lane status
> tables for `/soap-in/`, `/soap-out/`, `/xml-in/`, `/xml-out/`.

## Response shape (XTR-generated errors)

```json
{ "error": "<stable_code>", "message": "<human message>", ...extras }
```

Extras depend on variant:

| Variant | Extras |
|---|---|
| `upstream_soap_fault` | `code`, `string`, `detail` (SOAP lane only; `detail` present only when `expose_soap_fault_detail: true`) |
| `request_too_large` / `upstream_body_too_large` | `limit` (byte cap exceeded) |
| `template_not_found` | `group`, `service` (each clipped to 256 chars — audit-v2 F-XTR-3) |
| `method_not_allowed` | `method`, `group`, `service` |

## Status table

| Status | `error` | Lane | Cause |
|---|---|---|---|
| `400` | `invalid_json_body` | SOAP DSLs (`/:group/:service`) | Request body is not a JSON object (audit-v2 FN3). Empty body and `{}` are the zero-param path and stay 200. |
| `400` | `unsupported_encoding` | Raw-XML (`/xml-in/…`) | XML declaration names an encoding other than `UTF-8` / `utf8` (any case). ERRU §5.7.2. |
| `400` | `malformed_xml` | Raw-XML (`/xml-in/…`) | XML prolog isn't well-formed (missing/misplaced/duplicated `<?xml?>`, repeated declaration, non-1.0/1.1 version, text outside the root). |
| `401` | `unauthorized` | SOAP + REST DSLs (`/:group/:service`), `/soap-out/`, `/xml-out/` | `XTR_INTER_SERVICE_TOKEN` is set at boot and the request did not present `Authorization: Bearer <TOKEN>`. `/health` and `/api` are never gated. |
| `404` | `template_not_found` | Both | No DSL matched `/<group>/<service>`. |
| `404` | `not_found` | `/api` | `observability.expose_openapi: false` and someone hit `/api` (audit-v2 F-XTR-1). Body does **not** enumerate any DSL. |
| `405` | `method_not_allowed` | Both | Inbound HTTP method doesn't match the DSL's declared `method:`. SOAP DSLs are POST-only; REST DSLs contract whichever method they declare. |
| `413` | `request_too_large` | All | Body exceeded `limits.max_request_bytes`. |
| `422` | `schema_invalid` | Raw-XML (`/xml-in/…`, `/xml-out/…`) | A message or request fails XSD validation when the sidecar sets the direction's `validate:` to `enforce`. Body carries up to 10 `{path, rule, line, schemaFile, schemaLine}` violations; no message values are included (never leaks secrets or paths). |
| `500` | `template_expansion_failed` | SOAP | Handlebars render error at request time. Startup validation catches most; anything reaching here is exotic (e.g. runtime helper failure). |
| `500` | `keystore_load_failed` | Both | `.p12` couldn't be read/parsed. Both lanes share the mTLS identity, so either triggers this. |
| `500` | `internal_error` | Both | Unexpected. REST DSLs also 500 with this when `security_server:` is missing — the doctor's `fatal-rest-no-security-server` catches this at deploy time. Check the log line. |
| `500` | `validator_failed` | Raw-XML | Internal XSD validator error. XTR fails closed: no backend call, no forward, no outbound. Details go to the log only, never the response body. |
| `502` | `upstream_http_error` | SOAP | Upstream returned non-2xx AND the body wasn't a parseable SOAP Fault. REST lane does not translate — see "REST passthrough" below. |
| `502` | `upstream_soap_fault` | SOAP | Upstream returned `<Fault>` (on HTTP 200 OR wrapped in HTTP 5xx). Both SOAP 1.1 and 1.2 shapes handled. Control chars in `code` / `string` become `U+FFFD` (audit-v2 FN2). |
| `502` | `upstream_xml_parse_error` | SOAP | Response wasn't valid XML. Includes XXE-guard rejections (custom entities) and nesting-depth cap. |
| `502` | `upstream_body_too_large` | All | Response exceeded `limits.max_response_bytes`. Connection torn down. |
| `502` | (raw-XML outbound unreachable) | Raw-XML (`/xml-out/…`) | Peer refused, timed out, or returned no body. Peer URL is not leaked in the error. |
| `504` | `upstream_timeout` / (bare) | All | Upstream didn't respond within `limits.request_timeout_secs` **or** the handler-level `TimeoutLayer` fired at `limits.request_timeout_secs + 5s` (audit-v2 §6.2). |
| `508` | `loop_detected` | Raw-XML (`/xml-in/…`) | A message arrived with `x-xtr-forwarded: 1` and the lane would forward it again. Forward cycle guard — a forwarded message that goes to a backend is accepted. |
| `599` | `xtr_offline` | All outbound (SOAP + REST + Raw-XML) | `XTR_OFFLINE=true` is set. Every outbound dispatch short-circuits before reqwest; no real upstream is contacted. Pentest / break-test safety (audit-v2 FN-LOG-3). |

## REST passthrough — upstream 4xx / 5xx

REST DSLs are transparent proxies. When the upstream (or the
Security Server) returns a non-2xx status, XTR passes it through
**unchanged**:

- Same status code (401, 403, 404, 500, whatever).
- Same body bytes (usually the provider's JSON error shape).
- All X-Road response headers preserved — including
  `X-Road-Error`, which distinguishes provider-service errors from
  Security-Server errors per X-Road REST §4.6.

This means callers see the provider's own error format on the
wire, not an `XtrError` envelope. Only XTR-generated failures
(the table above) use the `{"error": …, "message": …}` shape.

To tell "was this XTR or the upstream?":

- **XTR-generated**: response body is JSON matching the shape at
  the top of this page.
- **Upstream passthrough**: response body is whatever the
  provider service returned; the `X-Road-Error` header will name
  the X-Road component that flagged the failure (if any).

## What XTR does NOT return

- `403` — XTR has no built-in authorisation. REST-lane upstream
  403s pass through; they're the provider's, not XTR's.
- `429` — no built-in rate limiting. Terminate at the ingress.

Note: `401 unauthorized` IS emitted when `XTR_INTER_SERVICE_TOKEN`
is set at boot — see the status table above. Default off,
backward-compatible behind Ruuter.

## See also

- [Configuration](./configuration.md) — tune the limits.
- [Security Server setup](./security-server.md) — for `keystore_load_failed` and mTLS-specific failures.
- [REST passthrough](./rest-passthrough.md) — for the `X-Road-Error` header semantics and per-header response behaviour.
