# REST passthrough

XTR fronts X-Road REST services the same way it fronts SOAP ones:
a hand-written DSL file per service, mounted at
`<method> /<group>/<service>`. Callers speak plain HTTP to XTR;
XTR speaks mTLS to the Security Server on their behalf.

Wire behaviour implements the
[X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).
Each claim below carries the spec section it maps to.

## When to reach for the REST lane

- Your provider speaks X-Road REST (Population Register `/r1/`,
  most services published post-2019).
- You want a single component (XTR) to hold the mTLS identity for
  your whole stack instead of every caller managing its own.
- You need transparent passthrough — no JSON reshape, no
  translation, provider's response bytes untouched.

Use the SOAP lane instead when the provider publishes a WSDL — see
[WSDL folder-drop](./wsdl-ingestion.md) for auto-generation.

## DSL shape

```yaml
# DSL/rr/isikud.yml   →   /rr/isikud on the axum surface
kind: rest
method: GET                          # DSL contract — non-GET → 405
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  # Versioning lives INSIDE path (spec §4.1). There is no
  # separate service_version field.
  path: /v1/isikud
# Optional query filter:
#   omitted    → forward every query key unmodified (spec §4.5 default)
#   []         → drop every query key
#   [k1, k2]   → allow-list
allowed_query_params:
  - personalCode
forward_body: true                   # default; set false to send empty body upstream
```

Parent directory becomes the URL group (`rr`); filename stem
becomes the service (`isikud`). Same convention as the SOAP lane.

### Field reference

| Field | Required | Purpose |
|---|---|---|
| `kind: rest` | ✓ | Selects the REST lane. Omitted → SOAP. |
| `method:` | ✓ | HTTP method the DSL contracts. Inbound mismatch → `405`. |
| `target.member_class` | ✓ | Provider identity — from RIA registration. |
| `target.member_code` | ✓ | Provider identity. |
| `target.subsystem_code` | ✓ | Provider identity. |
| `target.service_code` | ✓ | The service code registered under the subsystem. |
| `target.path` |   | Appended after `{service_code}`. Leading slash optional. Include any versioning (`/v1/…`). |
| `allowed_query_params` |   | Absent → forward all (spec §4.5 default). `[]` → drop all. Non-empty → allow-list. |
| `forward_body` |   | Default `true`. Set `false` for methods that must not carry a body. |

## What XTR does on the wire

Given the DSL above and this inbound request:

```
GET http://xtr/rr/isikud?personalCode=38001011234&extra=preserved
Accept: application/json
X-Road-UserId: EE38001011234
```

XTR builds and sends:

```
GET https://<security-server>/r1/ee-test/GOV/70008440/rr/dde/v1/isikud?personalCode=38001011234
Accept: application/json
X-Road-Client: ee-test/GOV/70008440/<your-subsystem>
X-Road-Id: <fresh-uuid>
X-Road-UserId: EE38001011234
```

Per-header semantics:

| Header | Direction | Behaviour |
|---|---|---|
| `X-Road-Client` | outbound | Mandatory (§4.3). XTR always sets this from config. Inbound values are stripped — callers cannot spoof identity. |
| `X-Road-Id` | outbound | If caller sets one, forwarded verbatim. Else XTR generates a UUID (§4.3). |
| `X-Road-UserId` | outbound | Forwarded verbatim from caller. XTR never synthesises it. |
| `Accept` | outbound | Forwarded unmodified (§4.3). |
| `Content-Type` | outbound | Forwarded unmodified (§4.3). |
| `Cache-Control`, `Pragma` | outbound | Forwarded unmodified (§4.3). |
| User-defined (`X-Custom-*` etc.) | outbound | Forwarded unmodified (§4.3). |
| `Host` | outbound | Stripped — reqwest sets it from the SS URL. |
| Hop-by-hop (`Connection`, `TE`, `Upgrade`, `Transfer-Encoding`, `Keep-Alive`, `Proxy-*`, `Trailer`) | outbound | Stripped. |
| `X-Road-Service`, `X-Road-Request-Hash`, `X-Road-Request-Id`, `X-Road-Error`, `X-Road-Id` | inbound (response) | Forwarded to caller (§4.3 response headers). |

## URL construction (spec §4.1)

```text
<SS URL>/r1/{instance}/{member_class}/{member_code}/{subsystem_code}/{service_code}{path}
```

Each identifier segment is percent-encoded per §4.2 — a
`service_code` literally containing `/` becomes `%2F`. XTR uses
RFC 3986 "unreserved" (`A-Za-z0-9-._~`) as the safe set; every
other character is encoded.

## Passthrough response

The upstream response passes through **as-is**: same status, same
`Content-Type`, same body bytes, plus all X-Road response headers
the provider Security Server sets (spec §4.3). Hop-by-hop response
headers are stripped.

Upstream 4xx / 5xx responses pass through untouched — including
the `X-Road-Error` header, which lets the caller distinguish
provider-side errors from Security-Server-side errors per spec
§4.6.

Failures internal to XTR (413 request too large, 502 upstream I/O
error, 504 timeout, 405 method mismatch) still surface as the
standard `XtrError` JSON envelope. See
[Failure modes](./failure-modes.md).

## HTTP redirects

Per spec §4.4, X-Road does not follow redirects. XTR pins its
reqwest client to `redirect::Policy::none()` — 3xx responses reach
the caller verbatim so the caller decides whether to follow.

## Required configuration

Every REST DSL routes through the Security Server; there is no
plain-REST bypass. When any REST DSL is loaded, `xtr.yaml` MUST
carry:

```yaml
security_server:
  url: https://<your-ss>:5500/                 # spec §4.7: HTTPS only
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD
  # Almost always needed. Real X-Road SS certs live behind an
  # operator-managed private CA that isn't in the system trust
  # store. Set to the PEM CA bundle if you get "unknown issuer"
  # handshake errors.
  trust_ca_path: /app/ssl/xroad-ca.pem
```

See [Security Server setup](./security-server.md) for how to
obtain the PKCS12 + CA bundle.

`xtr-on-rust doctor --strict` catches the common issues at deploy
time — see [Doctor & migration](./doctor.md) for the REST-lane
rule catalogue.

## Trust boundary and shared mTLS

The whole point of the REST lane is that XTR — not each caller —
holds the mTLS identity to the Security Server:

```
Ruuter    ─plain HTTP──►  XTR  ──mTLS──►  X-Road SS  ──►  RR REST service
Muu app   ─plain HTTP──►  XTR  ──mTLS──►  X-Road SS  ──►  LR SOAP service
```

XTR is the only component in the stack that ever talks mTLS to the
Security Server, for either SOAP or REST. Callers behind XTR need
plain-HTTP reachability to XTR only — no per-caller PKCS12
keystore, no per-caller SS route. Certificate rotation is a
single-component change.

## Identifier character restrictions

X-Road REST §4.8 restricts identifier values to
`A-Za-z0-9'()+,-.=?`. XTR's loader accepts non-conforming values
(so operators can experiment) but the doctor flags them as
`weak-rest-identifier-charset`. Real Security Servers may reject
them.

## Non-goals

- **Prefix-mount / wildcard passthrough** — one DSL file still
  maps to one URL. Watch for a follow-up if you need to expose a
  whole REST service under a single prefix.
- **Response translation** — no JSON reshape, no XML→JSON adapter.
  The response is opaque.
- **Auto-generation from OpenAPI** — REST DSLs are hand-written.
  SOAP DSLs get WSDL-driven generation; there's no equivalent for
  REST today.
