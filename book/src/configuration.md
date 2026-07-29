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

client_data:                             # → {{{generate.client}}}
  member_class: GOV                      # GOV / COM / NGO / NEE
  member_code: "<your-registry-code>"
  subsystem_code: <your-subsystem>

wsdl_watch_dir: ./wsdl                   # auto-generate DSLs from WSDLs
                                         # (unset = feature off)

security_server:                         # X-Road mTLS routing
  url: "https://<your-ss-fqdn>:5500/"
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD

limits:                                  # resource ceilings
  max_request_bytes: 1048576             # 1 MiB inbound
  max_response_bytes: 16777216           # 16 MiB upstream
  request_timeout_secs: 30
```

## Fields

| Field | Default | Purpose |
|---|---|---|
| `dsl_path` | `./DSL` | Directory walked for `*.yml` / `*.yaml` DSL files. |
| `port` | `8080` | HTTP listen port. |
| `xroad_instance` | `ee-test` | Injected as `{{generate.instance}}`. |
| `xroad_protocol_version` | `"4.0"` | Injected as `{{generate.protocol_version}}`. |
| `client_data.member_class` | `""` | Injected into `<xroad:client>`. |
| `client_data.member_code` | `""` | Injected into `<xroad:client>`. |
| `client_data.subsystem_code` | `""` | Injected into `<xroad:client>` (correctly spelled — fixes JVM bug #1). |
| `wsdl_watch_dir` | absent | Feature off when unset. See [WSDL folder-drop](./wsdl-ingestion.md). |
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
