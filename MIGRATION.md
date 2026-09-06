# Migrating XTR from `0.1.0-rc.2` → `0.2.0-rc`

**Audience**: operators upgrading a live deployment, and LLMs
assisting them.  
**Fastest path**: run [`xtr-on-rust doctor`](#the-doctor-recipe)
against your `xtr.yaml`, fix every FATAL / BREAK finding it
prints, deploy. Everything else on this page is what the
doctor knows, written out longform for humans.

---

## TL;DR

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  turnerrainer/xtr:0.2.0-rc doctor --strict
```

- **exit 0** — safe to deploy as-is.
- **exit 1 with FATAL** — the service will not boot or a
  critical property is off; fix before deploying.
- **exit 1 under `--strict`** — everything works, but a
  stronger security posture is available. Address WEAK
  findings on your schedule.

Machine-readable variant for CI / LLM pipelines:

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  turnerrainer/xtr:0.2.0-rc doctor --format json
```

Emits an array of `{severity, code, field, headline, rationale, recovery}`
objects. The `code` field is stable across releases — pin
your CI rules to those, not to headlines.

---

## The doctor recipe

`xtr-on-rust doctor` is a new subcommand shipped in the same
image as the server. It reads `xtr.yaml` the same way the
server does (`--config` flag → `XTR_CONFIG` env → `./xtr.yaml`
→ built-in defaults) and emits one **finding** per issue in
one of four severities:

| Severity | Meaning | Exit code |
|---|---|---|
| **FATAL** | Server will not boot with this config, or a critical property is broken. | `1` |
| **BREAK** | Behaviour changed vs `0.1.0-rc.2` and this config lands on the losing side. Set the named recovery flag if you need bit-for-bit equivalence. | `1` (currently no BREAK-only checks; reserved for future minor bumps) |
| **WEAK** | Currently works, but a stronger posture is available. Recommended for public deployments. | `0` normally, `1` with `--strict` |
| **INFO** | Positive observations (successful checks, resource ceilings). | `0` |

### Flags

```
xtr-on-rust doctor [flags]
  --config PATH          Explicit xtr.yaml path (else default search order)
  --format text|json     Output format (default text)
  --strict               Promote WEAK findings to exit code 1
```

### Sample output

Against the shipped `xtr.yaml` (Ariregister demo posture):

```
xtr-on-rust doctor — v0.2.0-rc
------------------------------------------------------------

WEAK (1)
  • [weak-wsdl-allowlist-empty] wsdl.upstream_host_allowlist is empty
    field:    wsdl.upstream_host_allowlist
    why:      Without a pinned host list, a WSDL that resolves
    why:      an attacker-controlled hostname to a metadata IP
    why:      still slips past the url_guard's literal-IP check.
    why:      Pinning the set of upstreams closes the DNS lane.
    recover:
      xtr.yaml:
        wsdl:
          upstream_host_allowlist:
            - ariregxmlv6.rik.ee
            - jvis.envir.ee

INFO (3)
  • [info-config-xroad-protocol-ok] ...
  • [info-config-source] ...
  • [info-limits-summary] ...

------------------------------------------------------------
Summary: 0 FATAL, 0 BREAK, 1 WEAK, 3 INFO
```

Exit 0 (safe to deploy) — one WEAK finding you may want to
address on your own schedule.

---

## Breaking changes reference

Each subsection: what changed, who's affected, how to detect
it in your config or in your consumers' behaviour, and the
one-line recovery flag.

### 1. SOAP fault response shape

**Change**: JSON body for `502 upstream_soap_fault` now omits
the `detail` field by default and caps `string` (faultstring)
at 200 characters. The `message` field is shortened.

**Before** (`0.1.0-rc.2`):
```json
{
  "error": "upstream_soap_fault",
  "message": "upstream returned SOAP Fault (Server): DB error: connect to postgres://admin:PASS@10.0.0.5/prod failed",
  "code": "Server",
  "string": "DB error: connect to postgres://admin:PASS@10.0.0.5/prod failed",
  "detail": { "stack": "at internal.jsp:42" }
}
```

**After** (`0.2.0-rc`, default):
```json
{
  "error": "upstream_soap_fault",
  "message": "upstream returned SOAP Fault (Server)",
  "code": "Server",
  "string": "DB error: connect to postgres://admin:PASS@10.0.0.5/prod fai… (truncated)"
}
```

The server logs still carry the full detail at `warn!` level
via structured `tracing` fields (`fault_code`, `fault_string`,
`fault_detail`).

**Who's affected**:

- Any REST consumer reading `response.detail` — that key is
  now absent (JSON `undefined`, not `null`).
- Anyone with an alert that regexes the `message` field for
  the old `"(<code>): <string>"` shape.
- Anyone whose observability was reading the full
  `faultstring` for parsing.

**Detect in your consumers**:

```bash
# From a captured 502 body, check whether `detail` is present
jq 'has("detail")' captured.json
# → true = you were reading detail. Set the recovery flag or
#   move the parsing to server logs (fault_detail field).
```

**Recovery** (bit-for-bit equivalence with 0.1.0-rc.2):

```yaml
# xtr.yaml
expose_soap_fault_detail: true
```

Doctor code: `weak-error-expose-soap-fault-detail` (flagged
when the flag is `true`).

---

### 2. `xroad_protocol_version` enum validation

**Change**: values other than `"4.0"` or `"4.1"` now hard-fail
startup with an error naming the bad value and the accepted
set. Empty string, typos, or a value you set experimentally
will now refuse to boot.

**Before**: any string accepted, injected into every
`<xroad:protocolVersion>` element. Requests failed at the
Security Server with a cryptic error.

**After**:
```
Error: xroad_protocol_version '9.9' is not one of the accepted values ["4.0", "4.1"]
```

**Who's affected**: only operators who typo'd this field or
set it to something outside the accepted set.

**Detect**:
```bash
grep "^xroad_protocol_version:" xtr.yaml
# Value must be exactly "4.0" or "4.1" (quoted).
```

**Recovery**:
```yaml
xroad_protocol_version: "4.0"   # or "4.1"
```

Doctor code: `fatal-config-xroad-protocol-invalid`.

---

### 3. X-Road sidecar identity validation

**Change**: `<wsdl>.meta.yaml` sidecars that declare
`member_class`, `member_code`, or `subsystem_code` must match
the corresponding field under `client_data` in `xtr.yaml`.
Any mismatch → whole WSDL is skipped with a WARN log.

Empty `client_data` fields (default state) skip the check
per-field, so an operator who hasn't onboarded to X-Road yet
still gets all their endpoints — but see the WEAK finding
about empty client_data.

**Who's affected**:

- The shipped `xtr.yaml` used to ship placeholder text
  (`"<your-registry-code>"`). If you're upgrading and left
  the placeholder in, sidecar identity validation will
  reject every real sidecar.
- Multi-tenant deployments where a shared WSDL mount
  contains sidecars claiming different X-Road identities.

**Detect**:
```bash
# 1. Real placeholders sitting in prod config:
grep -E '"<[^>]+>"' xtr.yaml
# → any output = FATAL under doctor

# 2. Existing sidecars naming a different identity than config:
for meta in wsdl/**/*.meta.yaml; do
  echo "=== $meta ==="
  grep -E "member_(class|code)|subsystem_code" "$meta"
done
# Compare against xtr.yaml's client_data.
```

**Recovery**:

Option A — align sidecar values with config:
```yaml
# xtr.yaml
client_data:
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
```
Then verify every sidecar declares the same triple.

Option B — remove identity fields from sidecars; keep only
overrides that make sense per-endpoint (`service_code`,
`service_url`):
```yaml
# wsdl/vendor/foo.meta.yaml
service_code: fooOperation
service_url: https://foo-vendor.example/soap
```

Doctor codes:
- `fatal-client-data-placeholder-member_code`,
- `fatal-client-data-placeholder-subsystem_code`,
- `weak-client-data-empty` (all three identity fields empty).

---

### 4. URL guard on WSDL upstreams

**Change**: every URL discovered in a WSDL `<soap:address
location=…>` or metadata sidecar `service_url:` override
is validated at ingest. Rejects:

- private / loopback / link-local / CGNAT / ULA IP ranges
- IPv4-mapped-IPv6 (`::ffff:169.254.169.254` — the metadata
  bypass)
- non-http(s) schemes (`file://`, `gopher://`, etc.)
- plain `http://` unless `wsdl.allow_http_upstream: true`

Rejected URLs are dropped from the DSL — the WSDL still
loads and its operations still generate, but they'll need
the Security Server route at request time.

**Who's affected**:

- Any operator whose WSDL corpus points at a private-IP
  upstream inside their network (e.g. `http://10.0.0.5/`).
- Anyone using plain HTTP upstreams (typically local
  development mocks).

**Detect**:
```bash
# Scan every WSDL for upstream URLs that will be rejected
grep -rEho '<soap:address location="[^"]+"' wsdl/ \
  | sed 's/^.*location="//;s/"$//' \
  | while read url; do
      case "$url" in
        http://10.*|http://192.168.*|http://172.1[6-9].*|http://172.2*.*|http://172.3[0-1].*)
          echo "PRIVATE $url" ;;
        http://169.254.*)
          echo "METADATA $url" ;;
        http://*) echo "PLAIN-HTTP $url" ;;
        *) : ;;  # https or other schemes — case-by-case
      esac
    done
```

**Recovery**:

For legitimate internal-network upstreams:
```yaml
# xtr.yaml
wsdl:
  allow_http_upstream: true              # allow plaintext HTTP
  upstream_host_allowlist:               # pin to internal hosts
    - internal-soap.example
```

**Private IPs cannot be recovered** — that's by design (SSRF
guard). If your upstream lives on `10.0.0.5`, front it with
a proxy on a routable hostname.

Doctor codes: `weak-wsdl-allow-http`, `weak-wsdl-allowlist-empty`.

---

## Non-breaking but observable

### HTTP client no longer decompresses (M2)

Both executors now build reqwest with `.no_gzip()`,
`.no_brotli()`, `.no_deflate()`. reqwest's default WAS to
decompress transparently.

If any upstream sends `Content-Encoding: gzip`
unconditionally, XTR now hands the compressed bytes to the
XML parser and it will error with `upstream_xml_parse_error`.

**Detect** (against a mock or in staging):
```bash
curl -sv -X POST http://localhost:8080/<group>/<service> \
  -H content-type:application/json -d '{}' 2>&1 \
  | grep -Ei "content-encoding|upstream_xml_parse_error"
```

No recovery flag yet. If you hit this, open an issue.

### XML depth cap 512 → 128

SOAP envelopes rarely nest > 20 levels; 128 leaves ~10x
headroom. If your particular upstream nests deeper than 128,
XTR now returns `upstream_xml_parse_error` with an
"XML nesting depth exceeded (128)" message.

### Schema-include filename restriction

WSDL `<xsd:include schemaLocation="…"/>` filenames now
restricted to `[A-Za-z0-9._-]+`. Symlinks under the WSDL
directory are rejected outright. Filenames outside the
charset or symlink-organised XSDs will be silently dropped
from parsing.

**Detect**:
```bash
find wsdl -name "*.xsd" -type l -print   # symlinks under wsdl/
find wsdl -type f -name "*.xsd" | grep -vE '^[/A-Za-z0-9._-]+$'
```

---

## Doctor rule catalogue

Every rule the doctor knows, by `code`. Codes are stable
across the 0.2.x line — pin CI to these, not to headlines.

| Code | Severity | Fires when |
|---|---|---|
| `fatal-config-xroad-protocol-invalid` | FATAL | `xroad_protocol_version` outside `{"4.0", "4.1"}` |
| `fatal-client-data-placeholder-member_code` | FATAL | `client_data.member_code` contains `<` or `>` |
| `fatal-client-data-placeholder-subsystem_code` | FATAL | `client_data.subsystem_code` contains `<` or `>` |
| `fatal-keystore-env-missing` | FATAL | `security_server` configured, env var absent |
| `fatal-keystore-env-empty` | FATAL | `security_server` configured, env var set to empty string |
| `fatal-keystore-file-missing` | FATAL | `security_server.keystore_path` doesn't exist on disk |
| `weak-wsdl-allow-http` | WEAK | `wsdl.allow_http_upstream: true` |
| `weak-wsdl-allowlist-empty` | WEAK | `wsdl.upstream_host_allowlist: []` **and** `wsdl_watch_dir` is set |
| `weak-error-expose-soap-fault-detail` | WEAK | `expose_soap_fault_detail: true` |
| `weak-client-data-empty` | WEAK | all three `client_data.*` fields empty |
| `weak-limits-request-too-generous` | WEAK | `limits.max_request_bytes > 16 MiB` |
| `weak-limits-response-too-generous` | WEAK | `limits.max_response_bytes > 128 MiB` |
| `weak-limits-timeout-too-long` | WEAK | `limits.request_timeout_secs > 300` |
| `weak-paths-dsl-missing` | WEAK | `dsl_path` doesn't exist on disk |
| `weak-paths-wsdl-watch-missing` | WEAK | `wsdl_watch_dir` set but doesn't exist |
| `info-*` | INFO | positive observations; never affects exit code |

---

## For LLM assistants helping an operator upgrade

Copy-paste this into your Claude / GPT session:

> I'm upgrading `turnerrainer/xtr` from `0.1.0-rc.2` to
> `0.2.0-rc`. Please help me plan the upgrade.
>
> 1. Here's my current `xtr.yaml`:
>    ```yaml
>    <paste the whole file>
>    ```
> 2. Here's my `docker-compose.yml` / `Deployment` manifest:
>    ```yaml
>    <paste>
>    ```
> 3. Here's what my downstream consumers do with the JSON
>    response body from `POST /:group/:service`:
>    - `<describe consumers, e.g. "log the entire body via
>      Filebeat, then Kibana queries look for
>      body.detail.stack">`
>
> Read `MIGRATION.md` at the root of the `turnerrainer/XTR`
> repo. Then:
>
> - Predict what `xtr-on-rust doctor --strict` will report
>   against my `xtr.yaml`. List FATAL / BREAK / WEAK codes.
> - For each finding, tell me the minimum-change diff to fix
>   it AND the recovery-flag alternative that preserves
>   `0.1.0-rc.2` behaviour.
> - For breaking change #1 (SOAP fault shape), tell me
>   whether my consumers as described will break, and give
>   me either the recovery flag OR a jq/Elasticsearch
>   migration query I need to run.
> - Give me a `docker run` command to run the actual doctor
>   against the file to verify your prediction.

---

## For CI pipelines

Recommended pre-deploy gate:

```yaml
# .github/workflows/xtr-config-gate.yml
name: XTR config gate
on:
  pull_request:
    paths:
      - 'xtr.yaml'
      - 'wsdl/**'
jobs:
  doctor:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - name: XTR doctor
        run: |
          docker run --rm \
            -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" \
            -v "$PWD/wsdl:/app/wsdl:ro" \
            turnerrainer/xtr:0.2.0-rc doctor --strict --format json \
          | tee doctor.json
      - name: Assert no FATAL
        run: |
          fatal=$(jq '[.[] | select(.severity=="FATAL")] | length' doctor.json)
          if [ "$fatal" -gt 0 ]; then
            echo "::error::doctor found $fatal FATAL findings"
            jq '.[] | select(.severity=="FATAL")' doctor.json
            exit 1
          fi
```

Add `--strict` to the run command to gate on WEAK findings
too when your team is ready for that posture.

---

## Rollback

Every change on this branch is contained in the container
image `turnerrainer/xtr:0.2.0-rc`. The prior image
`turnerrainer/xtr:0.1.0-rc.2` (digest
`sha256:61d441d00f75`) remains published on Docker Hub +
ghcr.io and is still cosign-signed. Roll back with a pod-spec
image swap; no data migration is involved (XTR is stateless).
