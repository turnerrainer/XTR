# Configuration

## Where the file lives

Search order:

1. `--config <path>` CLI flag
2. `XTR_CONFIG=<path>` env var
3. `./xtr.yaml` or `./xtr.yml` in the working directory
4. Built-in defaults (no file required)

Boot log says which won:

```
INFO xtr_on_rust: loaded config from ./xtr.yaml
```

## Full annotated `xtr.yaml`

```yaml
dsl_path: ./DSL                          # tree of *.yml DSL files (SOAP + REST)
port: 8080

xroad_instance: ee-test                  # → {{generate.instance}} (SOAP envelope)
                                         # → X-Road-Client instance (REST header)
xroad_protocol_version: "4.0"            # → {{generate.protocol_version}} (SOAP only)
                                         # must be "4.0" or "4.1"

client_data:                             # X-Road identity XTR presents
  member_class: GOV                      # GOV / COM / NGO / NEE
  member_code: "70000000"                # your organisation's registry code
  subsystem_code: my-subsystem           # what you registered with RIA
  # → {{{generate.client}}} in SOAP envelopes
  # → X-Road-Client: {instance}/{class}/{code}/{subsystem} on REST

wsdl_watch_dir: ./wsdl                   # auto-generate SOAP DSLs from WSDLs
                                         # (unset = feature off, hand-written only)
                                         # REST-lane is NOT affected — REST DSLs are
                                         # always hand-written.

wsdl:                                    # WSDL ingestion trust boundary (SOAP lane only)
  allow_http_upstream: false             # opt-in for plaintext http upstreams
  upstream_host_allowlist: []            # optional hostname pinning
  # upstream_host_allowlist:
  #   - ariregxmlv6.rik.ee
  #   - jvis.envir.ee

expose_soap_fault_detail: false          # echo raw upstream fault to REST callers
                                         # (SOAP lane only — REST lane doesn't
                                         # translate faults; upstream passes through)
                                         # NOTE: control chars in code/string are
                                         # ALWAYS replaced with U+FFFD (audit-v2 FN2).

observability:
  expose_openapi: true                   # audit-v2 F-XTR-1 / FN5. Default true for
                                         # backwards compat. Flip false in
                                         # untrusted-network deployments — the
                                         # OpenAPI spec enumerates every DSL group
                                         # + operation + upstream URL, a
                                         # service-discovery map for anyone planning
                                         # an unauth probe. When disabled, /api
                                         # returns 404 (JSON).

security_server:                         # X-Road Security Server routing (mTLS)
  # Required by REST DSLs and any SOAP DSL without `service:`.
  # Unused if you only run direct-HTTPS SOAP DSLs like Ariregister.
  url: https://out.test.x-tee.ee:5500/   # YOUR Security Server, port 5500 (message)
  keystore_path: /app/ssl/xtr-client.p12 # PKCS12 client identity
  keystore_password_env: XTR_KEYSTORE_PASSWORD
  # Almost always needed for real X-Road: the SS's TLS cert is
  # issued by an operator-managed private CA that isn't in the
  # system trust store. Point at the CA bundle PEM.
  trust_ca_path: /app/ssl/xroad-ca.pem   # optional; PEM CA bundle

limits:                                  # resource ceilings
  max_request_bytes: 1048576             # 1 MiB inbound  → 413 on overflow
  max_response_bytes: 16777216           # 16 MiB upstream → 502 on overflow
  request_timeout_secs: 30               # per outbound   → 504 on overflow
```

## Fields

Grouped by which lane needs them. "Both" means the field is
consulted by SOAP and REST DSLs alike.

### Runtime

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `dsl_path` | `./DSL` | Both | Directory walked for `*.yml` / `*.yaml` DSL files. |
| `port` | `8080` | Both | HTTP listen port. |
| `limits.max_request_bytes` | `1048576` (1 MiB) | Both | Inbound REST body cap. Overflow → 413. |
| `limits.max_response_bytes` | `16777216` (16 MiB) | Both | Upstream response cap. Overflow → 502, connection torn down. |
| `limits.request_timeout_secs` | `30` | Both | Per outbound request. Timeout → 504. |

### X-Road identity

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `xroad_instance` | `ee-test` | Both | SOAP: `{{generate.instance}}`. REST: instance segment of URL + `X-Road-Client`. |
| `xroad_protocol_version` | `"4.0"` | SOAP | `{{generate.protocol_version}}`. **Boot-validated**: must be `"4.0"` or `"4.1"` (audit-v1 M1). Not sent on REST — the REST protocol has its own version (`r1`, hard-coded). |
| `client_data.member_class` | `""` | Both | SOAP: `<xroad:client>`. REST: `X-Road-Client`. Empty skips sidecar identity check. |
| `client_data.member_code` | `""` | Both | Same as above. |
| `client_data.subsystem_code` | `""` | Both | Correctly spelled (fixes JVM bug #1). Same as above. |

### WSDL folder-drop (SOAP only)

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `wsdl_watch_dir` | absent | SOAP | Feature off when unset. See [WSDL folder-drop](./wsdl-ingestion.md). |
| `wsdl.allow_http_upstream` | `false` | SOAP | When false, `<soap:address>` URLs must be `https://`. Set true only for local test setups. Audit-v1 C1. |
| `wsdl.upstream_host_allowlist` | `[]` | SOAP | Optional. When non-empty, every WSDL upstream host must appear on the list. Closes the DNS-rebinding lane. Audit-v1 C1. |

### SOAP fault exposure

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `expose_soap_fault_detail` | `false` | SOAP | When false, upstream SOAP `Fault.detail` is stripped from REST responses and `faultstring` is capped at 200 chars; server logs still carry the full detail at `warn!` level. Set true only inside trusted environments. Audit-v1 H3 + audit-v2 FN2 (control chars in `code` / `string` are always replaced with U+FFFD, regardless of this flag). Does not apply to REST DSLs — REST faults come from the provider service, not from XTR. |

### Observability

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `observability.expose_openapi` | `true` | Both | Audit-v2 F-XTR-1. When true, `GET /api` returns the auto-generated OpenAPI 3.1 spec. When false, `GET /api` returns 404 with a structured JSON body that does NOT enumerate any DSL group. Recommended `false` when XTR is reachable from untrusted networks (the spec is a service-discovery map for anyone planning an unauth probe of `/:group/:service`). |

### Security Server (mTLS)

| Field | Default | Lane | Purpose |
|---|---|---|---|
| `security_server` | absent | Both | Required by REST DSLs. Required by SOAP DSLs that omit `service:`. Absent → those DSLs error at request time. |
| `security_server.url` | required if section set | Both | URL of YOUR Security Server (not the central authority's). Must be `https://` (see doctor rule `fatal-rest-ss-not-https`). Standard port is `5500` for the message channel; `4000` is the admin UI (do not use). |
| `security_server.keystore_path` | required if section set | Both | PKCS12 identity file for mTLS. Same file serves both lanes. |
| `security_server.keystore_password_env` | `XTR_KEYSTORE_PASSWORD` | Both | Env var name to read the PKCS12 password from. Never a default value — fixes JVM bug #16. |
| `security_server.trust_ca_path` | absent | Both | Optional PEM CA bundle used to verify the Security Server's TLS cert. Real X-Road SS certs are typically behind an operator-managed private CA that isn't in the system trust store; without this the handshake fails with `unknown issuer`. |

## Environment variables

| Variable | Purpose |
|---|---|
| `XTR_CONFIG` | Alternative path to `xtr.yaml` (bypasses cwd search). |
| `XTR_KEYSTORE_PASSWORD` | Password for the PKCS12 identity. Required whenever `security_server:` is set. |
| `XTR_OFFLINE` | Audit-v2 FN-LOG-3 test-safety switch. Truthy values (`1`, `true`, `yes`, `on`, case-insensitive) intercept EVERY outbound SOAP + REST dispatch and return HTTP **599** `xtr_offline` before any reqwest call. Doctor emits WEAK `weak-offline-mode-active` when set. Intended for pentest / break-test runs — **never leave enabled in production**. |
| `RUST_LOG` | `tracing_subscriber` filter (`info`, `debug`, `xtr_on_rust=trace`, …). |

## Validating your config before deploying

Run `xtr-on-rust doctor` (shipped in the same image) — it walks
the loaded config, emits FATAL / BREAK / WEAK / INFO findings, and
exits non-zero when something will fail at boot or when a stronger
security posture is available. See
[Doctor & migration](./doctor.md) for the full rule catalogue —
including the REST-lane codes that flag missing SS, non-HTTPS SS
URL, empty target fields, and identifier-charset issues.

## Startup validation

Every SOAP DSL's Handlebars envelope is compiled at boot. A
malformed template blows up on startup with the offending file
path — not on the first live request. REST DSLs have no template
and are validated against required-fields presence + spec
identifier charset at load time.

## No hot reload

Config, DSLs, and WSDLs are read once at boot. Restart to apply
changes.
