# Doctor & migration

XTR ships an in-image config validator: `xtr-on-rust doctor`.
Run it against your `xtr.yaml` before every deploy — it flags
what will break at boot, what changed vs the previous minor
version, and where a stronger security posture is available.

The full migration guide lives in
[`MIGRATION.md`](https://github.com/turnerrainer/XTR/blob/dev/MIGRATION.md)
at the repo root (mirrored in this book under
[reference/migration](./reference/migration.md)).

## Recipe

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  -v "$(pwd)/DSL:/app/DSL:ro" \
  turnerrainer/xtr:rc doctor --strict
```

Mount the DSL tree too — several REST-lane rules only fire when
the doctor can see the loaded DSL files (e.g. it can only warn
about a missing Security Server if REST DSLs are present).

## Findings model

| Severity | Meaning | Exit code |
|---|---|---|
| **FATAL** | Server will not boot with this config. | `1` |
| **BREAK** | Behaviour changed vs last minor and your config is on the losing side. Set the named recovery flag if you need bit-for-bit equivalence. | `1` |
| **WEAK** | Currently works, but a stronger posture is available. | `0` normally; `1` under `--strict` |
| **INFO** | Positive observations. | `0` |

The `code` field on every finding is stable — pin your CI rules
to those, not to headlines.

## Rule catalogue

Every code the doctor can emit, grouped by area.

### Startup validation (always checked)

| Severity | Code | Fires when |
|---|---|---|
| FATAL | `fatal-config-xroad-protocol-invalid` | `xroad_protocol_version` isn't `"4.0"` or `"4.1"` (audit-v1 M1). |
| INFO | `info-config-xroad-protocol-ok` | Protocol version accepted. |
| INFO | `info-config-source` | Which file was loaded (`--config` / `XTR_CONFIG` / `./xtr.yaml`). |
| INFO | `info-config-defaults` | No config file found; using built-in defaults. |
| INFO | `info-limits-summary` | Snapshot of resource ceilings that will apply. |

### X-Road identity (SOAP + REST)

| Severity | Code | Fires when |
|---|---|---|
| FATAL | `fatal-client-data-placeholder-member_code` | `client_data.member_code` still holds `<placeholder>` text. |
| FATAL | `fatal-client-data-placeholder-subsystem_code` | Same for `subsystem_code`. |
| WEAK | `weak-client-data-empty` | All three `client_data.*` fields empty — envelope/header will have no identity. |

### Security Server + mTLS (used by REST + Security-Server-routed SOAP)

| Severity | Code | Fires when |
|---|---|---|
| FATAL | `fatal-keystore-env-missing` | `security_server:` is set but its `keystore_password_env` variable is unset. |
| FATAL | `fatal-keystore-env-empty` | Env var is set but empty. |
| FATAL | `fatal-keystore-file-missing` | `keystore_path` doesn't exist on disk. |
| INFO | `info-keystore-env-present` | Env var resolved. |

### WSDL folder-drop (SOAP lane)

| Severity | Code | Fires when |
|---|---|---|
| WEAK | `weak-wsdl-allow-http` | `wsdl.allow_http_upstream: true` (audit-v1 C1). |
| WEAK | `weak-wsdl-allowlist-empty` | `wsdl_watch_dir` set but `wsdl.upstream_host_allowlist` is `[]`. |
| INFO | `info-wsdl-allowlist-pinned` | Non-empty allowlist. |

### SOAP fault exposure

| Severity | Code | Fires when |
|---|---|---|
| WEAK | `weak-error-expose-soap-fault-detail` | `expose_soap_fault_detail: true` (audit-v1 H3). |

### REST lane (issue #5)

| Severity | Code | Fires when |
|---|---|---|
| FATAL | `fatal-rest-no-security-server` | REST DSL(s) loaded but `security_server:` is unset. Every REST request would 500. |
| FATAL | `fatal-rest-ss-not-https` | `security_server.url` doesn't start with `https://` (X-Road REST §4.7). |
| FATAL | `fatal-rest-target-fields-missing` | A REST DSL has empty `target.member_class` / `member_code` / `subsystem_code` / `service_code`. |
| WEAK | `weak-rest-identifier-charset` | A REST DSL's target identifiers contain characters outside spec §4.8 (`A-Za-z0-9'()+,-.=?`). |
| INFO | `info-rest-lane-ready` | REST DSL(s) present and SS configured. |
| INFO | `info-rest-trust-ca-system` | Using system trust store for SS TLS — flag reminder to set `trust_ca_path` if the SS uses a private CA. |

### Resource limits

| Severity | Code | Fires when |
|---|---|---|
| WEAK | `weak-limits-request-too-generous` | `max_request_bytes` > 16 MiB. |
| WEAK | `weak-limits-response-too-generous` | `max_response_bytes` > 128 MiB. |
| WEAK | `weak-limits-timeout-too-long` | `request_timeout_secs` > 300. |

### Path checks

| Severity | Code | Fires when |
|---|---|---|
| WEAK | `weak-paths-dsl-missing` | `dsl_path` doesn't exist. |
| WEAK | `weak-paths-wsdl-watch-missing` | `wsdl_watch_dir` set but path doesn't exist. |

### Deployment hardening (audit-v2)

| Severity | Code | Fires when |
|---|---|---|
| WEAK | `weak-writable-rootfs-wsdl-folder-drop` | `wsdl_watch_dir` is set. Folder-drop needs a writable DSL dir, precluding `read_only: true` container rootfs (FLEET-STRONGHOLDS §7). Recovery text names the pre-generate-on-host workflow. |
| WEAK | `weak-offline-mode-active` | `XTR_OFFLINE` env var is truthy. Every outbound SOAP + REST dispatch will be short-circuited with HTTP 599. Intended for pentest / break-test runs; refuse to leave in production. |
| INFO | `info-no-caller-auth` | Always emitted. Reminder that XTR ships zero built-in caller authentication on `/:group/:service` — a reverse proxy or service mesh MUST gate the route in every deployment. Design property, not a fixable-in-config finding. |

> **Upgrade note — `doctor --strict` exit code change in audit-v2.**
>
> The shipped `xtr.yaml` sets `wsdl_watch_dir: ./wsdl` to make
> `docker compose up` work out of the box. That configuration
> now trips the new `weak-writable-rootfs-wsdl-folder-drop`
> WEAK — so any CI gate running `doctor --strict` against the
> shipping posture will start exiting `1`.
>
> Two legitimate paths forward:
>
> 1. **Hardened posture** — pre-generate DSLs on the host (bake
>    them into the container image or mount them read-only), set
>    `wsdl_watch_dir: null`, and enable `read_only: true` in the
>    container. WEAK count drops back to 0; `--strict` exit 0.
> 2. **Convenience posture** — keep folder-drop and drop the
>    `--strict` flag. Non-strict runs still exit 0 with WEAKs
>    reported for triage.
>
> Neither is wrong. The WEAK exists so the trade-off is
> visible, not to say folder-drop is broken.

## Machine-readable output

```bash
xtr-on-rust doctor --format json | jq '.[] | select(.severity == "FATAL")'
```

Stable schema per finding:

```json
{
  "severity": "FATAL",
  "code":     "fatal-rest-no-security-server",
  "field":    "security_server",
  "headline": "3 REST DSL(s) loaded but security_server is unset",
  "rationale": "...",
  "recovery":  "..."
}
```

## Recommended CI gate

```yaml
# .github/workflows/xtr-config-gate.yml
name: XTR config gate
on:
  pull_request:
    paths: [xtr.yaml, wsdl/**, DSL/**]
jobs:
  doctor:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - run: |
          docker run --rm \
            -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" \
            -v "$PWD/wsdl:/app/wsdl:ro" \
            -v "$PWD/DSL:/app/DSL:ro" \
            turnerrainer/xtr:rc doctor --strict --format json \
          | tee doctor.json
      - run: |
          fatal=$(jq '[.[] | select(.severity=="FATAL")] | length' doctor.json)
          [ "$fatal" -eq 0 ] || { echo "::error::$fatal FATAL finding(s)"; exit 1; }
```

See the [migration reference](./reference/migration.md) for the
exact breaking changes across minor versions with their recovery
flags.
