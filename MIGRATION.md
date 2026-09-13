# Migrating XTR

Three migration guides on this page:

- **`0.3.0-rc → 0.4.0-rc`** (audit-v2) — three small breaking
  changes on the response wire + a `doctor --strict` exit-code
  flip on the shipping posture. Read this first if you're
  upgrading from `0.3.x`.
- **`0.2.0-rc.1 → 0.3.0-rc`** (REST passthrough) — issue #5 is
  additive, `0.2.x` SOAP DSLs continue to work unchanged.
- **`0.1.0-rc.2 → 0.2.0-rc`** (audit-v1) — original audit-v1
  migration, retained as a canonical reference.

---

## `0.3.0-rc` → `0.4.0-rc`

**TL;DR** — audit-v2 hardening release. Three small
externally-visible behaviour changes on the response wire plus
one `doctor --strict` exit-code change on the shipping posture.
Everything else is either additive (opt-in config, new response
headers) or log-format-only.

### Breaking changes

#### 1. Malformed JSON body → HTTP 400

**What changed**: SOAP DSL invocations with a non-empty,
non-object JSON body used to silently degrade to empty-params
(200 + upstream call with zero substitutions). They now return
HTTP `400 invalid_json_body` **before** any outbound call is
issued.

**Before (`0.3.0-rc`)**:

```bash
$ curl -X POST http://xtr:8080/svc/op -H content-type:application/json -d 'null'
# → 200 (or 502 if upstream rejected the empty envelope)
```

**After (`0.4.0-rc`)**:

```bash
$ curl -X POST http://xtr:8080/svc/op -H content-type:application/json -d 'null'
# → 400
# { "error": "invalid_json_body",
#   "message": "invalid JSON body: expected a JSON object, got null" }
```

**Recovery**: the legitimate zero-param invocations are unchanged.
Send `{}` (still 200) or an empty body (still 200). If a caller
was relying on the old tolerant behaviour, fix the caller —
h2ck.me flagged the amplification lane (FN3) as MEDIUM.

**Why this fired**: an attacker could send garbage on the XTR
wire and get XTR to make a real mTLS-authenticated outbound
call against a real X-Road service. The upstream's 500 then
attributed to XTR (the mTLS-authenticated caller), not the
anonymous attacker.

#### 2. SOAP fault fields sanitised

**What changed**: The `code` and `string` fields inside an
`upstream_soap_fault` JSON response body now have every C0
control character (except tab) and DEL replaced with `U+FFFD`
(Unicode REPLACEMENT CHARACTER). This applies on both paths —
the default-strip path and the opt-in
`expose_soap_fault_detail: true` path. The `expose_soap_fault_detail`
flag governs whether the `detail` block is included; it does
NOT give the upstream a byte-transparent channel into the
caller.

**Before (`0.3.0-rc`)**:

```json
{ "error": "upstream_soap_fault",
  "code": "Server",
  "string": "auth failed\r\nFORGED-LINE" }
```

**After (`0.4.0-rc`)**:

```json
{ "error": "upstream_soap_fault",
  "code": "Server",
  "string": "auth failed��FORGED-LINE" }
```

**Recovery**: a client parsing the visible message content is
unaffected — legitimate fault text doesn't carry control chars.
If a client was byte-transparent (extracting a stack trace with
literal `\r\n` from the string), switch to `detail` field
parsing (JSON already escapes control chars in transit).

**Why this fired**: h2ck.me FN2 residual — a malicious or
compromised upstream could pack CRLF / NUL / ANSI ESC into the
fault fields, poisoning downstream terminal renderers or log
shippers that display the JSON error body.

#### 3. `doctor --strict` exit code flips on the shipping posture

**What changed**: The new `weak-writable-rootfs-wsdl-folder-drop`
rule fires whenever `wsdl_watch_dir` is set — which is the
default in the shipped `xtr.yaml` and in the "one-command demo"
recipe. `doctor` still exits 0 by default; `doctor --strict`
now exits 1 on this posture.

**Before (`0.3.0-rc`)**:

```bash
$ docker run --rm -v ./xtr.yaml:/app/xtr.yaml:ro turnerrainer/xtr:0.3.0-rc doctor --strict
# → exit 0
# Summary: 0 FATAL, 0 BREAK, 0 WEAK, 3 INFO
```

**After (`0.4.0-rc`)**:

```bash
$ docker run --rm -v ./xtr.yaml:/app/xtr.yaml:ro turnerrainer/xtr:0.4.0-rc doctor --strict
# → exit 1
# WEAK (1)
#   • [weak-writable-rootfs-wsdl-folder-drop] wsdl_watch_dir is set —
#     container rootfs cannot be read-only
# Summary: 0 FATAL, 0 BREAK, 1 WEAK, 4 INFO
```

**Recovery**: two legitimate paths, both documented in the
doctor's `recover:` block on the finding. Pick the one that
matches your deployment reality.

**Path A — Hardened posture** (fleet-baseline; recommended for
prod):

1. Pre-generate DSLs on the host (`xtr-on-rust` in a build
   stage that runs before the container boots), and bake them
   into the image OR mount them read-only.
2. In `xtr.yaml`: `wsdl_watch_dir: null` (or omit).
3. In `docker-compose.yml`: `read_only: true` with a
   `tmpfs: /tmp:64M` for scratch.
4. `doctor --strict` returns to exit 0.

**Path B — Convenience posture** (folder-drop stays enabled):

1. Keep `wsdl_watch_dir: ./wsdl` as-is.
2. In CI, run `doctor` (no `--strict`) — the WEAK is reported
   for triage but doesn't fail the gate.
3. Compose: NOT `read_only: true`. Add `cap_drop: [ALL]`,
   `no-new-privileges: true`, non-root UID as
   compensating controls per `FLEET-STRONGHOLDS.md` §7.

**Why this fired**: h2ck.me RUNTIME FN4 (MED). Folder-drop
needs a writable DSL dir, which conflicts with the
fleet-baseline `read_only: true` posture — an attacker with
code execution inside the container can persist to `/app`.

### Additive-but-observable

None of these require a config change. A strict caller could
still notice.

#### New response headers

Every response now carries seven additional headers:

```
content-security-policy: default-src 'none'; frame-ancestors 'none'
strict-transport-security: max-age=63072000; includeSubDomains; preload
x-frame-options: DENY
x-content-type-options: nosniff
referrer-policy: no-referrer
traceparent: 00-<32-hex-trace>-<16-hex-span>-01
x-trace-id: <32-hex-trace>
```

The middleware never overwrites a header set upstream (matters
for REST passthrough).

#### Handler-level 504 pathway

Previously, a slow handler-side step (handlebars expansion,
XML translate, etc.) could hang until the client gave up. A
new `tower_http::timeout::TimeoutLayer` caps the entire
handler pipeline at `limits.request_timeout_secs + 5s` — a
small grace on top of the outbound reqwest cap. Overflow
surfaces as HTTP 504 (matches the existing `UpstreamTimeout`
variant's status). Health checks that tolerated hangs may now
see 504.

#### One INFO access-log line per request

Structured line every request:

```
INFO http_request_completed method=POST route="/:group/:service"
     status=200 duration_us=1234 trace_id=abc12345…
```

Log volume rises accordingly; log-shippers may need a rate cap.
The `route` field is the matched pattern, not the raw URI — log
cardinality stays bounded regardless of caller-chosen path.

### New config fields (opt-in)

```yaml
# xtr.yaml
observability:
  expose_openapi: true       # default; flip false to hide /api
                             # from unauth callers
```

New env var:

| Var | Truthy → | Notes |
|---|---|---|
| `XTR_OFFLINE` | Every outbound short-circuits with HTTP 599 `xtr_offline` | Test-safety mode. Doctor emits WEAK `weak-offline-mode-active` when set. Never leave enabled in prod. Truthy values: `1`, `true`, `yes`, `on` (case-insensitive). |

### New doctor rules

| Sev | Code | Fires when |
|---|---|---|
| WEAK | `weak-writable-rootfs-wsdl-folder-drop` | `wsdl_watch_dir` is set (see Breaking Changes §3 above). |
| WEAK | `weak-offline-mode-active` | `XTR_OFFLINE` env var is truthy. |
| INFO | `info-no-caller-auth` | Always emitted — reminder that XTR ships no built-in caller auth on `/:group/:service`. |

### Doctor recipe

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  -v "$(pwd)/DSL:/app/DSL:ro" \
  turnerrainer/xtr:0.4.0-rc doctor --strict
```

- **exit 0** — safe to deploy as-is.
- **exit 1 with FATAL** — the service will not boot or a
  critical property is off; fix before deploying.
- **exit 1 under `--strict` with WEAK only** — everything
  works, but a stronger posture is available. If the WEAK is
  `weak-writable-rootfs-wsdl-folder-drop`, see Breaking Changes
  §3 above for the two legitimate recovery paths.

### For LLM assistants helping an operator upgrade

Ready-to-paste prompt:

```
I'm upgrading XTR from 0.3.0-rc to 0.4.0-rc. My current
xtr.yaml is:

<paste xtr.yaml>

My CI gate is:

<paste CI snippet, if any>

Given the audit-v2 breaking changes (malformed-JSON 400,
SOAP fault sanitiser, doctor --strict exit-code flip), what
changes do I need to make? Answer with:

1. Exact xtr.yaml diff.
2. Whether my CI gate stays --strict or drops it (justify).
3. Any caller-side changes if I depend on the pre-audit-v2
   tolerant behaviour.
```

---

## `0.2.0-rc.1` → `0.3.0-rc`

**TL;DR** — additive release. Existing SOAP DSLs work unchanged.
Two externally-visible changes worth reviewing before deploy.

### Externally-visible changes

1. **DSL `method:` is now enforced on both kinds.**
   Previously, non-POST requests to a SOAP DSL were routed to
   axum's built-in 405 (because the route was `POST /:group/:svc`).
   The router is now `any /:group/:svc` (needed for REST DSLs
   that declare `method: GET|PUT|DELETE`), so method mismatches
   surface as XTR's own structured `405 method_not_allowed`
   response.
   - **Symptom of the change**: `GET /some-soap-endpoint` now
     returns `{"error":"method_not_allowed","message":"..."}`
     with `405` status. Previously the same request got axum's
     bare `405 Method Not Allowed` with no body.
   - **No recovery flag needed** — no SOAP DSL should be receiving
     GETs in practice. If yours does, add a REST DSL for the GET
     path or fix the caller.

2. **New optional `security_server.trust_ca_path` config field.**
   Real X-Road Security Server TLS certs are typically issued by
   an operator-managed private CA. When the CA isn't in the
   system trust store, the mTLS handshake fails with `unknown
   issuer`. Point `trust_ca_path` at the CA bundle PEM.
   - **No recovery flag needed** — the field is optional; absent
     means "use system trust store" (unchanged 0.2 behaviour).
   - Applies to both SOAP and REST lanes.

### Added (opt-in, no impact if unused)

- **REST passthrough lane** (issue #5). DSL files may declare
  `kind: rest` and act as X-Road REST endpoints. See the "REST
  passthrough" chapter of the book for the operator-facing
  setup guide, or `book/src/rest-passthrough.md` in-repo.
- **Doctor rules for REST DSLs**: `fatal-rest-no-security-server`,
  `fatal-rest-ss-not-https`, `fatal-rest-target-fields-missing`,
  `weak-rest-identifier-charset`, plus two informational codes.
  Only fire when a REST DSL is loaded.
- **`XtrError::MethodNotAllowed`** — new error variant. Wire
  shape `{"error":"method_not_allowed","message":"..."}` with
  status `405`.

### Doctor recipe

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  -v "$(pwd)/DSL:/app/DSL:ro" \
  turnerrainer/xtr:0.3.0-rc doctor --strict
```

- **exit 0** — safe to deploy as-is.
- **exit 1 with FATAL** — the service will not boot or a
  critical property is off; fix before deploying.
- **exit 1 under `--strict`** — everything works, but a
  stronger security posture is available.

Mount the DSL tree too — several REST-lane rules only fire when
the doctor can see the loaded DSL files.

### Prompt template for LLM-assisted upgrade

```
I'm upgrading XTR from 0.2.0-rc.1 to 0.3.0-rc. My current
xtr.yaml is:

<paste xtr.yaml>

My DSL/ tree contains:

<paste `ls -R DSL/` output>

Please:
1. Tell me if the upgrade is safe (any SOAP DSL that receives
   non-POST requests? Any DSL kind change needed?).
2. Suggest whether I should set security_server.trust_ca_path.
3. Show the exact xtr.yaml diff I need.

Facts I want you to use:
- 0.3.0-rc adds a REST passthrough lane (kind: rest DSLs).
- SOAP DSLs work unchanged.
- Method mismatch on SOAP DSLs now returns structured 405.
- security_server.trust_ca_path is new + optional.
```

---

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
