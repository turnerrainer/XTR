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
dsl_path: ./DSL                          # tree of *.yml DSL files
port: 8080

xroad_instance: ee-test                  # → {{generate.instance}}
xroad_protocol_version: "4.0"            # → {{generate.protocol_version}}
                                         # (must be "4.0" or "4.1")

client_data:                             # → {{{generate.client}}}
  member_class: GOV                      # GOV / COM / NGO / NEE
  member_code: ""                        # e.g. "70000000"
  subsystem_code: ""                     # e.g. "myservice"

wsdl_watch_dir: ./wsdl                   # auto-generate DSLs from WSDLs
                                         # (unset = feature off)

wsdl:                                    # WSDL ingestion trust boundary
  allow_http_upstream: false             # opt-in for plaintext http upstreams
  upstream_host_allowlist: []            # optional hostname pinning
  # upstream_host_allowlist:
  #   - ariregxmlv6.rik.ee
  #   - jvis.envir.ee

expose_soap_fault_detail: false          # echo raw upstream fault to REST callers

security_server:                         # X-Road mTLS routing
  url: "https://<your-ss-fqdn>:5500/"
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD

limits:                                  # resource ceilings
  max_request_bytes: 1048576             # 1 MiB inbound
  max_response_bytes: 16777216           # 16 MiB upstream
  request_timeout_secs: 30
```

## Validating your config before deploying

Run `xtr-on-rust doctor` (shipped in the same image) — it
walks the loaded config, emits FATAL / BREAK / WEAK / INFO
findings, and exits non-zero when something will fail at boot
or when a stronger security posture is available. See
[Doctor & migration](./doctor.md) for the recipe.

## Fields

| Field | Default | Purpose |
|---|---|---|
| `dsl_path` | `./DSL` | Directory walked for `*.yml` / `*.yaml` DSL files. |
| `port` | `8080` | HTTP listen port. |
| `xroad_instance` | `ee-test` | Injected as `{{generate.instance}}`. |
| `xroad_protocol_version` | `"4.0"` | Injected as `{{generate.protocol_version}}`. **Boot-validated**: must be one of `"4.0"` / `"4.1"` (audit-v1 M1). |
| `client_data.member_class` | `""` | Injected into `<xroad:client>`. Empty skips sidecar identity check. |
| `client_data.member_code` | `""` | Injected into `<xroad:client>`. Empty skips sidecar identity check. |
| `client_data.subsystem_code` | `""` | Injected into `<xroad:client>` (correctly spelled — fixes JVM bug #1). Empty skips sidecar identity check. |
| `wsdl_watch_dir` | absent | Feature off when unset. See [WSDL folder-drop](./wsdl-ingestion.md). |
| `wsdl.allow_http_upstream` | `false` | When false, `<soap:address>` URLs must be `https://`. Set true only for local test setups. Audit-v1 C1. |
| `wsdl.upstream_host_allowlist` | `[]` | Optional. When non-empty, every WSDL upstream host must appear on the list. Closes the DNS-rebinding lane on top of the literal-IP guard. Audit-v1 C1. |
| `expose_soap_fault_detail` | `false` | When false, upstream SOAP `Fault.detail` is stripped from REST responses and `faultstring` is capped at 200 chars; server logs still carry the full detail at `warn!` level. Set true only inside trusted environments. Audit-v1 H3. |
| `security_server` | absent | DSLs that omit `service:` will error at request time when this is unset. |
| `security_server.url` | required if section set | URL of YOUR Security Server (not the central authority's). |
| `security_server.keystore_path` | required if section set | PKCS12 identity file for mTLS. |
| `security_server.keystore_password_env` | `XTR_KEYSTORE_PASSWORD` | Env var name to read password from. Never a default value — fixes JVM bug #16. |
| `limits.max_request_bytes` | `1048576` (1 MiB) | Inbound REST body cap. Overflow → 413. |
| `limits.max_response_bytes` | `16777216` (16 MiB) | Upstream response cap. Overflow → 502, connection torn down. |
| `limits.request_timeout_secs` | `30` | Per outbound request. Timeout → 504. |

## Environment variables

| Variable | Purpose |
|---|---|
| `XTR_CONFIG` | Alternative path to `xtr.yaml` (bypasses cwd search). |
| `XTR_KEYSTORE_PASSWORD` | Password for the PKCS12 identity. Required if `security_server:` is set. |
| `RUST_LOG` | `tracing_subscriber` filter (`info`, `debug`, `xtr_on_rust=trace`, ...). |

## Startup validation

Every DSL's Handlebars envelope is compiled at boot. A malformed
template blows up on startup with the offending file path — not on
the first live request.

## No hot reload

Config, DSLs, and WSDLs are read once at boot. Restart to apply
changes.
