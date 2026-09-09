# Failure modes

Every HTTP status XTR emits for its own errors, with the stable
`error` code and cause. Upstream 4xx/5xx on the REST lane behave
differently — see "REST passthrough" below.

## Response shape (XTR-generated errors)

```json
{ "error": "<stable_code>", "message": "<human message>", ...extras }
```

Extras depend on variant:

| Variant | Extras |
|---|---|
| `upstream_soap_fault` | `code`, `string`, `detail` (SOAP lane only; `detail` present only when `expose_soap_fault_detail: true`) |
| `request_too_large` / `upstream_body_too_large` | `limit` (byte cap exceeded) |

## Status table

| Status | `error` | Lane | Cause |
|---|---|---|---|
| `404` | `template_not_found` | Both | No DSL matched `/<group>/<service>`. |
| `405` | `method_not_allowed` | Both | Inbound HTTP method doesn't match the DSL's declared `method:`. SOAP DSLs are POST-only; REST DSLs contract whichever method they declare. |
| `413` | `request_too_large` | Both | Body exceeded `limits.max_request_bytes`. |
| `500` | `template_expansion_failed` | SOAP | Handlebars render error at request time. Startup validation catches most; anything reaching here is exotic (e.g. runtime helper failure). |
| `500` | `keystore_load_failed` | Both | `.p12` couldn't be read/parsed. Both lanes share the mTLS identity, so either triggers this. |
| `500` | `internal_error` | Both | Unexpected. REST DSLs also 500 with this when `security_server:` is missing — the doctor's `fatal-rest-no-security-server` catches this at deploy time. Check the log line. |
| `502` | `upstream_http_error` | SOAP | Upstream returned non-2xx AND the body wasn't a parseable SOAP Fault. REST lane does not translate — see "REST passthrough" below. |
| `502` | `upstream_soap_fault` | SOAP | Upstream returned `<Fault>` (on HTTP 200 OR wrapped in HTTP 5xx). Both SOAP 1.1 and 1.2 shapes handled. |
| `502` | `upstream_xml_parse_error` | SOAP | Response wasn't valid XML. Includes XXE-guard rejections (custom entities) and nesting-depth cap. |
| `502` | `upstream_body_too_large` | Both | Response exceeded `limits.max_response_bytes`. Connection torn down. |
| `504` | `upstream_timeout` | Both | Upstream didn't respond within `limits.request_timeout_secs`. |

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

- `400` — malformed SOAP request bodies are treated as "no
  params". Not an error.
- `401` / `403` — XTR has no built-in auth. Put auth in front
  (reverse proxy, or a Ruuter DSL layer). REST-lane upstream 401s
  pass through; they're the provider's, not XTR's.
- `429` — no built-in rate limiting.

## See also

- [Configuration](./configuration.md) — tune the limits.
- [Security Server setup](./security-server.md) — for `keystore_load_failed` and mTLS-specific failures.
- [REST passthrough](./rest-passthrough.md) — for the `X-Road-Error` header semantics and per-header response behaviour.
