# Failure modes

Every HTTP status XTR emits, with the stable `error` code and cause.

## Response shape

```json
{ "error": "<stable_code>", "message": "<human message>", ...extras }
```

Extras depend on variant:

| Variant | Extras |
|---|---|
| `upstream_soap_fault` | `code`, `string`, `detail` |
| `request_too_large` / `upstream_body_too_large` | `limit` (byte cap exceeded) |

## Status table

| Status | `error` | Cause |
|---|---|---|
| `404` | `template_not_found` | No DSL matched `/<group>/<service>`. |
| `413` | `request_too_large` | Body exceeded `limits.max_request_bytes`. |
| `500` | `template_expansion_failed` | Handlebars render error at request time. (Startup validation catches most.) |
| `500` | `keystore_load_failed` | `.p12` couldn't be read/parsed. mTLS path only. |
| `500` | `internal_error` | Unexpected — check the log line. |
| `502` | `upstream_http_error` | Upstream returned non-2xx AND the body wasn't a parseable SOAP Fault. |
| `502` | `upstream_soap_fault` | Upstream returned `<Fault>` (on HTTP 200 OR wrapped in HTTP 5xx). Both SOAP 1.1 and 1.2 shapes handled. |
| `502` | `upstream_xml_parse_error` | Response wasn't valid XML. Includes XXE-guard rejections (custom entities) and nesting-depth cap. |
| `502` | `upstream_body_too_large` | Response exceeded `limits.max_response_bytes`. Connection torn down. |
| `504` | `upstream_timeout` | Upstream didn't respond within `limits.request_timeout_secs`. |

## What XTR does NOT return

- `400` — malformed request bodies are treated as "no params". Not an error.
- `401` / `403` — XTR has no built-in auth. Put auth in front (reverse proxy, or a Ruuter DSL layer).
- `429` — no built-in rate limiting.

## See also

- [Configuration](./configuration.md) — tune the limits.
- [Security Server setup](./security-server.md) — for `keystore_load_failed` and mTLS-specific failures.
