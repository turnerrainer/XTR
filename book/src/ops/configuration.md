# Configuration

XTR reads one YAML config file at boot. Every field is optional; unset
fields inherit safe defaults.

## Resolution priority

1. `--config <path>` CLI flag
2. `XTR_CONFIG=<path>` env var
3. `./xtr.yaml` or `./xtr.yml` in the working directory
4. Built-in defaults (no file needed)

The startup log tells you which source was chosen:

```
INFO xtr_on_rust: loaded config from ./xtr.yaml
# or
INFO xtr_on_rust: using built-in defaults (no xtr.yaml found)
```

## Full annotated example

```yaml
# xtr.yaml
dsl_path: ./DSL                          # directory tree of DSL files
port: 8080                               # HTTP listen port

xroad_instance: ee-test                  # X-Road instance code
xroad_protocol_version: "4.0"            # bump when RIA does (task 005)

client_data:                             # your X-Road identity
  member_class: GOV                      # GOV / COM / NGO / NEE
  member_code: "<your-registry-code>"    # organisation registry code
  subsystem_code: <your-subsystem>       # per your RIA registration

security_server:                         # mTLS to X-Road SS (optional)
  url: "https://<your-ss-fqdn>:5500/"
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD   # required env var name

limits:                                  # resource ceilings (task 011)
  max_request_bytes: 1048576             # 1 MiB inbound REST body
  max_response_bytes: 16777216           # 16 MiB upstream response
  request_timeout_secs: 30               # per outbound request
```

## Fields

| Field | Default | Purpose |
|---|---|---|
| `dsl_path` | `./DSL` | Directory walked recursively for `*.yml` / `*.yaml` DSL files. |
| `port` | `8080` | HTTP listen port. |
| `xroad_instance` | `ee-test` | Injected as `{{generate.instance}}` in envelopes. |
| `xroad_protocol_version` | `"4.0"` | Injected as `{{generate.protocol_version}}` in envelopes. |
| `client_data.member_class` | `""` | Injected into `<xroad:client>` element. |
| `client_data.member_code` | `""` | Injected into `<xroad:client>` element. |
| `client_data.subsystem_code` | `""` | Injected into `<xroad:client>` element. Note: **correctly spelled** — fixes JVM bug #1 (`sybsystem-code`). |
| `security_server` | `None` | Absent → DSLs that omit `service:` will error at request time (they need the SS). |
| `security_server.url` | required if section present | URL of YOUR SS (not the central authority's). |
| `security_server.keystore_path` | required if section present | PKCS12 identity file for mTLS. |
| `security_server.keystore_password_env` | `XTR_KEYSTORE_PASSWORD` | Env var name to read the password from. Never a default value — fixes JVM bug #16. |
| `limits.max_request_bytes` | `1048576` (1 MiB) | Inbound REST body cap. Overflow → 413. |
| `limits.max_response_bytes` | `16777216` (16 MiB) | Upstream response cap. Overflow → 502. Body is streamed and torn down on breach. |
| `limits.request_timeout_secs` | `30` | Per outbound request. Timeout → 504. |

## Startup validation

XTR validates every DSL's Handlebars envelope at load time; a bad
template blows up at boot with the offending file path. See
[Failure modes](./failure-modes.md).

## No hot reload

Config is read once at boot. Restart the process to pick up changes.
That's a deliberate choice — matches Ruuter's dev-only hot-reload
gating for the same reasons (see Ruuter's `book/src/ops/hot-reload.md`).

## See also

- [Environment variables](./env.md)
- [X-Road Security Server setup](./xroad-security-server.md)
- [Failure modes](./failure-modes.md)
