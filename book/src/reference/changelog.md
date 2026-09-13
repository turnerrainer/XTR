# Changelog

All notable changes to XTR-on-Rust will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0-rc] - 2026-09-13

Fourth minor release. Closes the h2ck.me audit-v2 residuals
(RUNTIME / LOG / PUBLIC-EXPOSURE break-tests) and adopts six
`FLEET-STRONGHOLDS.md` patterns. Test count: **225** (was 156
at 0.2.0-rc, 197 at 0.3.0-rc). Ten security / hardening PRs
(#10–#19) plus two documentation PRs (#20, #21) land on this
version.

### Breaking changes vs `0.3.0-rc`

Small surface, all in the audit-v2 fix branch. Full recovery
recipes in `MIGRATION.md` §"0.3.0-rc → 0.4.0-rc".

1. **Malformed JSON body → HTTP 400** (was: silently downgraded
   to empty params). Callers sending `null` / `[]` / `42` /
   truncated JSON to zero-param SOAP DSLs and getting 200 now
   receive `{"error":"invalid_json_body","message":"..."}`.
   Legitimate zero-param calls (empty body, explicit `{}`) are
   unaffected.
2. **SOAP fault fields sanitised**. Control chars (C0 range
   except tab, plus DEL) in the JSON body's `code` / `string`
   fields render as `U+FFFD` — even when
   `expose_soap_fault_detail: true`. A caller parsing a
   stack-trace-in-`string` still gets the text, escaped.
3. **`doctor --strict` exit code flips 0 → 1 on the shipping
   posture.** The new `weak-writable-rootfs-wsdl-folder-drop`
   rule fires whenever `wsdl_watch_dir` is set — which is the
   default in the shipped `xtr.yaml`. CI gates running
   `doctor --strict` will need to either (a) drop `--strict` or
   (b) adopt the hardened posture (pre-generate DSLs on host,
   set `wsdl_watch_dir: null`, enable `read_only: true`).
   Neither path is wrong; the WEAK exists so the trade-off is
   visible.

Additive-but-observable (no config change needed; a strict
caller could notice):

- Every response now carries five default security headers
  (`content-security-policy`, `strict-transport-security`,
  `x-frame-options`, `x-content-type-options`, `referrer-policy`)
  plus `traceparent` + `x-trace-id`.
- Handler-level timeout at `limits.request_timeout_secs + 5s` —
  previously slow handler-side steps (handlebars, XML translate)
  could hang; now cap at HTTP 504.
- One INFO access-log line per request (`http_request_completed`
  with method / route / status / duration / trace_id).

### Added

- **`observability.expose_openapi`** config field (default `true`).
  When `false`, `GET /api` returns a structured 404 whose body
  does NOT enumerate any DSL group. Recommended `false` in
  untrusted-network deployments. Addresses h2ck.me audit-v2
  F-XTR-1 (MED) and FN5 (LOW). (PR
  [#13](https://github.com/turnerrainer/XTR/pull/13))
- **`XTR_OFFLINE` env var** (truthy: `1` / `true` / `yes` / `on`,
  case-insensitive). When set, every outbound SOAP + REST
  dispatch short-circuits before any `reqwest` call and returns
  HTTP `599` with `{"error":"xtr_offline"}`. Intended for
  pentest / break-test runs. Doctor emits `weak-offline-mode-active`
  when the flag is on so an operator who accidentally leaves it
  enabled sees it. Addresses h2ck.me audit-v2 FN-LOG-3 (CRIT
  operational, test-safety). (PR
  [#15](https://github.com/turnerrainer/XTR/pull/15))
- **Security headers middleware** — five default headers on every
  response (CSP `default-src 'none'; frame-ancestors 'none'`,
  2-year HSTS with preload, `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`).
  Middleware never overwrites a header set by an upstream REST
  passthrough. Adopts FLEET-STRONGHOLDS §5.1. (PR
  [#16](https://github.com/turnerrainer/XTR/pull/16))
- **Access log + W3C `traceparent` propagation** — one INFO line
  per request with method / matched-route / status / duration
  in microseconds / trace_id. Trace-id is honoured from an
  inbound valid `traceparent` header (cross-service correlation)
  or minted as a fresh v4 UUID. Response carries both
  `traceparent` and `x-trace-id`. Route pattern (not raw URI)
  is what's logged — log cardinality stays bounded. Adopts
  FLEET-STRONGHOLDS §1.2 + §1.6. (PR
  [#17](https://github.com/turnerrainer/XTR/pull/17))
- **Handler-level `TimeoutLayer`** — caps the entire handler
  pipeline (body parse + handlebars expand + upstream + XML
  translate) at `limits.request_timeout_secs + 5s`, surfacing
  as HTTP 504. Adopts FLEET-STRONGHOLDS §6.2. (PR
  [#19](https://github.com/turnerrainer/XTR/pull/19))
- **`XtrError::InvalidJsonBody`** (400) — new variant for
  malformed-JSON rejection. (PR
  [#14](https://github.com/turnerrainer/XTR/pull/14))
- **`XtrError::OfflineMode`** (599) — new variant for
  `XTR_OFFLINE` short-circuit. (PR
  [#15](https://github.com/turnerrainer/XTR/pull/15))
- **`Executor::is_offline()`** + **`with_offline_for_tests(bool)`**
  — public accessor for the doctor tool, and a test helper that
  avoids poking a process-global env var. (PR
  [#15](https://github.com/turnerrainer/XTR/pull/15))
- **Three new doctor rules**:
  - `weak-writable-rootfs-wsdl-folder-drop` (RUNTIME v1 FN4) —
    fires when `wsdl_watch_dir` is set; the writable-DSL-dir
    requirement conflicts with `read_only: true` container rootfs.
  - `info-no-caller-auth` (PUBLIC-EXPOSURE v1 F-XTR-2) — always
    emitted; reminder that XTR ships zero built-in caller
    authentication on `/:group/:service`.
  - `weak-offline-mode-active` (LOG v1 FN-LOG-3) — surfaces the
    `XTR_OFFLINE` env-var state.
  (PRs [#15](https://github.com/turnerrainer/XTR/pull/15),
  [#18](https://github.com/turnerrainer/XTR/pull/18))
- **New book page** `book/src/http-contract.md` — the full
  response-shape contract: middleware stack diagram, default
  headers, status-code table, JSON error-body shape, SOAP fault
  sanitiser semantics, handler timeout, XTR_OFFLINE mode.
  (PRs [#20](https://github.com/turnerrainer/XTR/pull/20),
  [#21](https://github.com/turnerrainer/XTR/pull/21))

### Security

- **FN-LOG-1 (HIGH) — CRLF log injection via URL path.**
  `TemplateNotFound { group, service }` and adjacent error
  variants used to `Display`-format user-controlled path
  segments into `tracing::warn!` lines. `POST /x/y%0d%0aFAKE`
  decoded the CRLF and split one log entry into two, enabling
  attacker-forged audit records. Now uses Debug-formatted
  structured fields so control chars render as escape
  sequences. Three regression tests in
  `tests/security_log_url_path_no_crlf_leak.rs`. (PR
  [#10](https://github.com/turnerrainer/XTR/pull/10))
- **FN-LOG-2 (MED) — ANSI escapes shipped under Docker /
  systemd.** `tracing_subscriber` now enables colour only when
  stderr is a TTY (`std::io::stderr().is_terminal()`); plain
  text is shipped to log-shippers, SIEM systems, and
  `docker logs` files. (PR
  [#10](https://github.com/turnerrainer/XTR/pull/10))
- **FN2 (MED/LOW) — SOAP fault control-char sanitiser.** A
  malicious upstream could pack CRLF / NUL / ANSI ESC into the
  JSON `code` / `string` fields of a fault response, poisoning
  downstream terminal renderers or log-shippers that display it.
  New `sanitize_fault_field()` in `src/error.rs` replaces every
  C0/DEL control char (except tab) with `U+FFFD` before it
  lands in the response body — on both the default-strip path
  and the opt-in `expose_soap_fault_detail: true` path. (PR
  [#11](https://github.com/turnerrainer/XTR/pull/11))
- **F-XTR-3 (LOW) — echoed-path clip.** `TemplateNotFound` and
  `MethodNotAllowed` variants now clip attacker-controlled
  `group` / `service` / `method` fields at 256 chars each in
  the JSON body. Bounds the response amplification factor a
  single unauth request can force. (PR
  [#12](https://github.com/turnerrainer/XTR/pull/12))
- **FN3 (MED) — malformed JSON rejected before upstream call.**
  Previously, malformed or non-object JSON bodies silently
  degraded to empty-params upstream calls. An attacker could
  use XTR as an amplifier — garbage on the XTR wire triggered
  real mTLS-authenticated outbound calls against real X-Road
  services. Now returns HTTP 400 `invalid_json_body` before
  any outbound is issued. (PR
  [#14](https://github.com/turnerrainer/XTR/pull/14))
- **FN-LOG-5 (LOW) — SOAP fault fields log-side hardening.**
  `fault_code` / `fault_string` fields switched from Display
  (`%`) to Debug (`?`) format in the `tracing::warn!` line, so
  control chars in an upstream fault can't split the log line.
  (PR [#11](https://github.com/turnerrainer/XTR/pull/11))

### Changed

- **Test count**: 225 (was 197 at 0.3.0-rc).
- **`hardened_config_exits_zero_and_reports_no_fatal_or_weak`
  integration fixture** now omits `wsdl_watch_dir` — the new
  writable-rootfs WEAK fires on folder-drop by design, so the
  "hardened baseline" test asserts the fleet §7 posture (DSLs
  pre-generated on host).
- **Middleware layering**: `TimeoutLayer` (innermost) →
  `security_headers` → `access_log` (outermost). Attaches
  `traceparent` before the response reaches the wire; the
  trace-id lands even on short-circuited handlers.

### Fleet-strongholds adoption checklist

Applied at `0.4.0-rc`:

- ✅ §1.1 ANSI off outside TTY
- ✅ §1.2 structured access log with trace-id
- ✅ §1.5 `{:?}` on user-controlled input (never `{}`)
- ✅ §1.6 W3C `traceparent` + `x-trace-id`
- ✅ §5.1 five default security headers
- ✅ §6.2 handler-level `TimeoutLayer`
- ✅ §9.1 `XTR_OFFLINE=true` test-safety mode

## [0.3.0-rc] - 2026-09-10

Third minor release. Ships the X-Road REST passthrough lane
(issue [#5](https://github.com/turnerrainer/XTR/issues/5)) — XTR
now fronts both X-Road SOAP and X-Road REST services from the
same DSL directory, over the same mTLS identity. Spec-compliant
per [X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).

Fully backwards-compatible with existing `0.2.x` SOAP DSLs. See
the migration reference in this book for the (small) surface of
externally-visible changes and their recovery flags.

### Added

- **REST passthrough lane** (issue
  [#5](https://github.com/turnerrainer/XTR/issues/5)). Spec-compliant
  implementation of the
  [X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).
  DSL files declare `kind: rest` with a `target:` block; XTR
  constructs the `/r1/{instance}/{class}/{code}/{subsystem}/{service_code}[{path}]`
  URL (§4.1) with percent-encoded identifier segments (§4.2), sets
  `X-Road-Client` (§4.3) and `X-Road-Id`, and forwards all inbound
  headers (Accept, Content-Type, Cache-Control, X-Road-UserId,
  user-defined) unmodified (§4.3). Query params pass unmodified by
  default (§4.5), with an optional DSL-level allow-list.
  Response returns as-is with all upstream X-Road response headers
  (`X-Road-Service`, `X-Road-Request-Hash`, `X-Road-Error`, etc.)
  passed to the caller. Redirects pinned to `Policy::none()` per
  §4.4. See the "REST passthrough" chapter of the mdBook for the
  operator-facing setup guide.
- **`security_server.trust_ca_path`** config field. Real X-Road
  Security Server TLS certs are typically issued by an
  operator-managed private CA that isn't in the system trust store;
  this field points at a PEM bundle so the mTLS handshake can
  verify the server cert. Applies to both SOAP and REST lanes.
- **`XtrError::MethodNotAllowed`** (`405`) — emitted when the
  DSL-declared method doesn't match the inbound HTTP method.
  Applies to both SOAP (POST-only) and REST DSLs. Previously
  axum routed non-POST to a bare 405; the new `any` route needs
  an explicit variant so the wire shape (`{error, message}`)
  stays consistent.
- **Doctor rules for the REST lane**: `fatal-rest-no-security-server`,
  `fatal-rest-ss-not-https`, `fatal-rest-target-fields-missing`,
  `weak-rest-identifier-charset` (per spec §4.8),
  `info-rest-lane-ready`, `info-rest-trust-ca-system`.
- **Full-mTLS integration test** (`tests/it_rest_mtls.rs`) —
  `rcgen` + `openssl` (PKCS12) + `tokio-rustls` (client-cert
  verifying server) drive the production `RestLaneExecutor::new`
  code path end-to-end with a real handshake. Complements the
  plain-HTTP router tests in `tests/it_rest_passthrough.rs`.

### Changed

- `XRoadTemplate` is now `{ method, kind: TemplateKind }` where
  `TemplateKind` is `Soap` or `Rest`. Existing DSL files without a
  `kind:` field deserialise as `Soap` — full backward compatibility.
- `POST /:group/:service` route widened to `any /:group/:service`.
  DSL `method:` is now enforced at the handler; mismatches return
  `405 method_not_allowed` for both SOAP and REST kinds.
- `Executor` gained `dispatch_rest()`; existing `dispatch()` renamed
  to `dispatch_soap()`. External callers were only via the router.
- The mTLS client builder is now a shared `build_mtls_client()`
  helper used by both `SecurityServerExecutor` (SOAP) and
  `RestLaneExecutor` (REST). Applies redirect policy, TLS floor,
  decompression posture, and CA bundle from one place.

## [0.2.0-rc.1] - 2026-09-06

Hotfix — Dockerfile ENTRYPOINT / CMD interaction broke the
subcommand shape documented in `MIGRATION.md` and
`book/src/doctor.md`. The recipe
`docker run --rm turnerrainer/xtr:0.2.0-rc doctor` was
supposed to invoke the doctor subcommand but instead tini
tried to exec a non-existent `doctor` binary — the tini
error surfaced was
`FATAL tini (7) exec doctor failed: No such file or directory`.
Discovered when local-testing the just-published
`0.2.0-rc` image against the operator flow.

### Fixed

- `Dockerfile`: `ENTRYPOINT ["/usr/bin/tini", "--", "/app/xtr-on-rust"]`
  + `CMD []`. Extra args to `docker run` now APPEND as argv to
  the binary instead of REPLACING CMD. Bare `docker run <image>`
  still boots the server (no argv → server path in main.rs).
- `tests/dockerfile_entrypoint.rs` (new): contract test parses
  the Dockerfile and refuses any shape where ENTRYPOINT doesn't
  pin the binary. Guards against this class of regression before
  publish.

Everything else about `0.2.0-rc` still applies — see below.

## [0.2.0-rc] - 2026-09-06

Third release candidate. Closes the h2ck.me pre-publication
audit (v1) — two Critical, four High, five Medium findings on
the WSDL trust boundary. Because several fixes changed
externally-visible behaviour (SOAP fault response shape most
notably), this ships as a minor bump, not a patch.

Snyk-driven base-image bump `debian:bookworm-slim` →
`debian:13.6-slim` (PR #1) is included; runtime-verified with
end-to-end TLS calls to real Ariregister.

### Migration from 0.1.0-rc.2

Read [`MIGRATION.md`](./migration.md) and run
`xtr-on-rust doctor` (new subcommand — see below) against
your `xtr.yaml`. The doctor prints a per-finding table
(FATAL / BREAK / WEAK / INFO) with exact recovery flags.

### Breaking changes vs 0.1.0-rc.2

Four externally-visible changes. All have recovery flags for
strict behaviour equivalence; the defaults were changed
because the safer posture is a better fit for a public
release.

1. **SOAP fault response shape** (H3). The JSON body for a
   `502 upstream_soap_fault` no longer includes the `detail`
   field; `string` (faultstring) is capped at 200 characters;
   `message` is shortened from
   `"upstream returned SOAP Fault (X): Y"` to
   `"upstream returned SOAP Fault (X)"`.
   - Server logs still carry the full fault detail at `warn!`
     level via structured `tracing` fields.
   - **Recover exact-equivalence** by setting
     `expose_soap_fault_detail: true` in `xtr.yaml`.

2. **`xroad_protocol_version` enum validation at boot** (M1).
   Values other than `"4.0"` or `"4.1"` now hard-fail startup
   with an error naming the bad value and the accepted set.
   - Default remains `"4.0"`; vast majority of configs
     unaffected. Empty string, typos, or a value set by an
     operator experimenting with a newer protocol will now
     refuse to boot.
   - **Recover** by setting `xroad_protocol_version` to one
     of the accepted values.

3. **X-Road sidecar identity validation** (H2). If a
   `<wsdl>.meta.yaml` sidecar declares `member_class`,
   `member_code`, or `subsystem_code`, they must equal the
   corresponding `client_data` field in `xtr.yaml`. Mismatch
   → whole WSDL is skipped with a WARN. Empty `client_data`
   fields (default state) skip the check per-field so
   pre-onboarding operators still get all endpoints.
   - The shipped `xtr.yaml` uses placeholder
     `member_code: "<your-registry-code>"`. Any real sidecar
     with a real member code will now be rejected against
     the placeholder — set `client_data` before deploying
     with sidecars.
   - **Recover** by either aligning sidecar values with
     config OR removing the identity fields from the sidecar
     (sidecar can still override `service_code` /
     `service_url`).

4. **URL guard on WSDL upstreams** (C1). Every URL discovered
   in a WSDL `<soap:address location=…>` or metadata sidecar
   `service_url:` override is validated at ingest. Private,
   loopback, link-local, CGNAT, ULA, IPv4-mapped-IPv6, and
   non-http(s) schemes are rejected; the offending URL is
   dropped from the DSL (WSDL operations then fall back to
   Security Server routing). Also, `http://` upstreams are
   rejected by default.
   - **Recover** by setting `wsdl.allow_http_upstream: true`
     for plaintext HTTP upstreams, or by adding legitimate
     internal hostnames to `wsdl.upstream_host_allowlist`.
     Private-IP upstreams cannot be recovered — that is by
     design.

### Behaviour changes worth calling out

Not breaking in the SemVer sense (unlikely to affect real
deployments) but observable if you're at an edge:

- **HTTP client no longer decompresses response bodies**
  (M2). Both executors now build the reqwest client with
  `.no_gzip()`, `.no_brotli()`, `.no_deflate()`. reqwest's
  default WAS to decompress transparently, which would let a
  16 MiB wire-body cap silently protect a much larger
  in-memory payload. If any upstream sends
  `Content-Encoding: gzip` unconditionally, XTR now
  surfaces the compressed bytes to the XML parser and it
  will error. No recovery flag; if you hit this, open an
  issue and we'll add one.
- **XML depth cap 512 → 128**. Real X-Road envelopes are
  single-digit-deep; 128 leaves ~10x headroom while keeping
  the debug-build test-thread stack safe.
- **Schema-include filenames restricted** to
  `[A-Za-z0-9._-]+`. Symlinks under the WSDL dir are also
  rejected. Any WSDL corpus that ships XSDs with non-ASCII
  filenames or symlink-organized includes now silently
  drops those includes.

### Added

- **`xtr-on-rust doctor` subcommand** — new. Validates the
  operator's `xtr.yaml` against the audit-v1 ruleset and the
  known breaking-change recovery matrix. Emits FATAL /
  BREAK / WEAK / INFO findings; exit code 1 on any FATAL
  (or on WEAK under `--strict`). See
  [`MIGRATION.md`](./migration.md) for the recipe.
- **`MIGRATION.md`** — machine-readable + human-readable
  guide for both operators and LLMs walking through the
  0.1 → 0.2 upgrade.

### Security — h2ck.me audit-v1 fixes (2026-09-05)

Pre-publication audit findings from `h2ck.me/projects/XTR/v1`.
Two Critical, four High, five Medium — all closed on this branch.

- **C1 (SSRF via WSDL upstream URL)** — new
  `src/wsdl/url_guard.rs` validates every URL discovered in a
  `<soap:address location=…/>` or metadata-sidecar
  `service_url:` override. Rejects `http` by default, private /
  loopback / link-local / CGNAT / ULA / v4-mapped-v6 ranges,
  and non-http(s) schemes. New config: `wsdl.allow_http_upstream`,
  `wsdl.upstream_host_allowlist`.
- **C2 (XML bomb safety net)** — added `MAX_XML_EVENTS = 100_000`
  per-document event budget in `src/translate/xml_to_json.rs`.
  Complements the existing depth cap (lowered from 512 → 128
  for debug-stack safety) and the pre-existing custom-entity
  rejection. Regression test bombs → error, no expansion.
- **H1 (path traversal in WSDL schema-include loader)** —
  `resolve_local_schema` now applies filename charset restriction,
  symlink rejection via `symlink_metadata`, and canonicalisation
  with `starts_with(wsdl_dir)` containment check.
- **H2 (X-Road client impersonation via sidecar)** — sidecar
  `member_class` / `member_code` / `subsystem_code` are now
  validated against `client_data` at load time. Mismatch →
  refuse to load the sidecar (WSDL is skipped with a WARN).
- **H3 (SOAP fault detail leak)** — `IntoResponse` for
  `UpstreamSoapFault` now strips `detail` and caps `faultstring`
  at 200 chars by default. Full detail always logged at
  `warn!` level for operators. Config
  `expose_soap_fault_detail: bool` re-enables the raw response
  for internal debugging.
- **H4 (TLS defaults not tested)** — both executors now pin
  `min_tls_version(TLS_1_2)` explicitly on the reqwest builder.
  Added `tests/tls_defaults_enforced.rs` integration test
  (post-h2ck.me-v1 nit): spins a self-signed TLS server via
  `rcgen` + `tokio-native-tls` on a random port, asserts a
  default-trust-store reqwest client refuses the handshake AND
  a bypass-flagged client succeeds against the same server
  (counter-test guards against false-positive greens from a
  broken server setup).
- **M1 (xroad_protocol_version typos)** — `AppConfig::validate()`
  called at boot; rejects any value not in `{"4.0", "4.1"}`
  with an error naming both the bad value and the accepted set.
- **M2 (gzip invariant)** — both executors set `.no_gzip()`,
  `.no_brotli()`, `.no_deflate()` so the 16 MiB wire-body cap
  in `read_bounded` stays meaningful regardless of upstream
  content-encoding.
- **M3 (attribute-safe Handlebars helper)** — registered
  `{{xml_attr foo}}` helper that emits `&quot; &apos; &lt; &gt; &amp;`
  for interpolation into XML attribute values. Default `{{foo}}`
  is still safe in element text.

New crate dependency: `url = "2"` (already a transitive of
`reqwest` — promoted to a direct dep for `url_guard.rs`).

## [0.1.0-rc.2] - 2026-07-29

Second release candidate. Adds runtime WSDL folder-drop
(task 013), ships real Ariregister WSDL + 34 companion XSDs
under `wsdl/ar/` as the canonical source of truth (yields 33
auto-generated `/ar/*` endpoints on every boot), and renames
all "SS" abbreviations to "Security Server" throughout code,
config, DSLs, docs, and task files.

### Added — Task 013 WSDL folder-drop

- `wsdl_watch_dir` config field. At boot, XTR scans
  `<dir>/<group>/*.wsdl`, parses each, and generates
  `DSL/<group>/<operation>.yml` per `wsdl:operation`.
- SOAP-1.1 document/literal parser in `src/wsdl/` — supports
  inline anonymous complexTypes, named top-level complexTypes
  (with lazy resolution + cycle guard), `xsd:include` via a
  local-filesystem loader (offline discipline preserved),
  `xsd:import` skipped as framework, `xsd:annotation` skipped
  as documentation. Bail-out-on-unsupported for `xsd:choice`,
  WSDL 2.0, RPC/encoded, MIME attachments.
- Per-op lenient: unresolvable input elements log WARN and
  skip that operation; sibling operations still generate.
- Deterministic YAML output — same WSDL always produces
  byte-equal bytes.
- Generated DSLs carry a marker header. Hand-written DSLs
  (no marker) always win on collision with a WARN log.
- Optional `<wsdl>.meta.yaml` sidecar opts into X-Road
  envelope wrapping (member_class/member_code/subsystem_code
  → auto-generated `<xroad:*>` header block).
- Generator recognises the X-Road `TURVASERVER` placeholder
  in `<soap:address location=…/>` and omits `service:` so
  the executor routes via `security_server:` instead.

### Added — WSDL as source of truth

- `wsdl/ar/` — real Ariregister WSDL + 34 companion XSDs
  (~180 KB) vendored into the repo.
- `xtr.yaml` (new) — default config that ships with the
  repo. `docker compose up` / `cargo run` now boots with
  33 Ariregister endpoints live via WSDL ingestion.
- `.gitignore` — `/DSL/ar/*.yml` ignored (regenerated per
  boot from the WSDL). Hand-written DSLs (like `DSL/xroad/*`)
  stay tracked.
- Removed the 4 previously hand-written Ariregister sample
  DSLs (`lihtandmed_v3`, `detailandmed_v2`,
  `ettevottegaSeotudIsikud_v1`, `tegelikudKasusaajad_v2`) —
  now auto-generated with WSDL-native param names (Estonian
  `ariregistri_kood` instead of English `reg_code`).

### Changed — SS → Security Server

Renamed every "SS" abbreviation to "Security Server" across
code, configs, DSLs, book chapters, task files, comments,
and CHANGELOG entries. Rationale: the "SS" abbreviation
carries a well-known historical reputation that reads
unprofessional in a European government-infrastructure
context. See `feedback_never_abbreviate_security_server.md`.

- `SsExecutor` → `SecurityServerExecutor`
- `src/executor/ss.rs` → `src/executor/security_server.rs`
- `Executor.ss` field → `Executor.security_server`
- All prose in `book/`, `docs/`, `tasks/`, comments — same.
- SOAP protocol literals unchanged (`SOAP-ENV:Server`,
  `env:Server` etc are external error codes and must
  stay as-is).

### Docs

- `book/src/ops/wsdl-ingestion.md` — folder layout, marker
  semantics, override rules, X-Road sidecar convention,
  "no admin HTTP endpoint" rationale.
- `book/src/dsl/adding-a-service.md` — reframed as override
  fallback path; WSDL-drop is primary now.

Verified: 82/0/0 tests, fmt clean, clippy -D warnings
clean, mdbook + linkcheck build clean, live smoke boots
with 35 endpoints (33 auto-generated Ariregister + 2
hand-written X-Road samples).

## [0.1.0-rc.1] - 2026-07-28

First publishable release candidate. Working REST → SOAP →
X-Road proxy in Rust, live-verified against public Ariregister
endpoints. Everything below in this section is what ships in
this tag.

### Added — Post-MVP hardening sweep (2026-07-28)

Landed tasks 003, 005, 010, 011, 012, and a follow-up security
sweep in a single day. Test count 29 → 51 (0 fail, 0 ignored).

**Task 010 — SOAP Fault detection**. HTTP 200 + `<soap:Fault>`
now maps to a structured 502 `upstream_soap_fault` with
`code` / `string` / `detail` top-level fields, instead of silently
being translated as a successful response. Handles SOAP 1.1 and
1.2 including namespace-prefixed variants and `xml:lang`-tagged
Reason elements.

**Task 011 — Request/response size caps + timeout config**. New
`limits:` config section (`max_request_bytes` 1 MiB,
`max_response_bytes` 16 MiB, `request_timeout_secs` 30). Inbound
overflow → 413 `request_too_large`; upstream overflow → 502
`upstream_body_too_large` with the connection torn down
immediately. Outbound responses read chunk-by-chunk via a new
`read_bounded` helper — bounded memory per request.

**Task 012 — JSON type coercion**. Bare integer leaves become
`Value::Number`; `true`/`false` become `Value::Bool`. Deliberate
non-goals with enforcing tests: no float coercion (precision loss
on `"3.10"`), no leading-zero coercion (`"007"` stays string —
those are opaque IDs), no case-insensitive booleans, `i64`
overflow keeps raw string, attributed-leaf `#text` stays string.

**Task 005 — Explicit X-Road protocol version in config**. New
`xroad_protocol_version: "4.0"` config field exposed as
`{{generate.protocol_version}}` in the Handlebars auto-context.
The two shipped X-Road DSL samples (`listMethods`,
`allowedMethods`) migrated to the auto-context variable —
protocol-version changes now require a single config line update
instead of touching every DSL.

**Task 003 — Content-Type + charset on outbound calls**. Closed
as landed with task 002 Phase D — both executors already set
`text/xml; charset=utf-8`; existing integration test already
captured + asserted it. Marker added to `done/`.

### Security sweep

**quick-xml 0.36 → 0.41**. `cargo audit` flagged two
high-severity DoS advisories (RUSTSEC-2026-0194 quadratic on
duplicate attribute names, RUSTSEC-2026-0195 unbounded
namespace-declaration allocation) — both fixed in 0.41. Both
directly relevant since XTR parses untrusted upstream XML on
every request; size caps alone don't help against the quadratic
runtime.

**XXE guard**. quick-xml 0.41 introduced `Event::GeneralRef`
for entity references outside the XML-predefined set. Character
references (`&#nnn;`, `&#xhh;`) resolve to Unicode codepoints
via a new `decode_char_ref` helper. Custom entities
(`&nbsp;`, `&copy;`) are rejected with an explicit
`XmlParseError` mentioning XXE risk — accepting them would
require a DOCTYPE, which is the XXE attack surface.

**Nesting-depth cap (MAX_NESTING_DEPTH = 512)** on
`parse_children`. Prior state: unbounded recursion — a document
with hundreds of thousands of `<a><a><a>…` levels blew the
stack. Real envelopes rarely exceed 10 levels; cap gives ~50x
headroom.

**Regression coverage** added: Handlebars single-pass re-render
safety, malformed-body handling (7 shapes), percent-encoded-slash
path traversal, XML nesting cap, hex character ref, custom
entity XXE guard.

Final audit posture: `cargo audit` 0 advisories, `cargo deny
check` green on advisories/bans/licenses/sources.

### Added — Task 002 MVP (v0.1.0-rc.2 candidate)

Working REST → SOAP → X-Road proxy per DESIGN.md §8. Implements
the module tree, HTTP surface, DSL loader, Handlebars expansion,
executor (plain + mTLS), XML → JSON translation, auto-generated
OpenAPI, and integration tests. 12 of the 17 JVM XTR bugs from
DESIGN.md §7 fixed:

  #1  subsystem_code (correctly spelled)
  #2  no @Value on statics — instance-field config
  #3  Handlebars: single-pass render with merged context
  #5  <xroad:client> element built correctly (no literal %s)
  #6  system trust store (no trust-all X509TrustManager)
  #7  response exposes both {body, headers}
  #8  route pattern is /:group/:service (not wildcard)
  #9  structured error responses ({error, message} + proper status)
  #10 /health endpoint
  #13 port 8080 everywhere (no 9010/9020/8080 confusion)
  #14 OpenAPI param type "string" (not "String")
  #15 no `Towarsd` typo
  #16 keystore password from env var, never a default

Modules added (src/*)
  * config/        — AppConfig with load_or_default (--config /
                     XTR_CONFIG / ./xtr.yaml search path)
  * dsl/           — XRoadTemplate, ServiceMap, loader::load_all,
                     handlebars::expand (unified single-pass render)
  * executor/      — PlainExecutor (system trust), SecurityServerExecutor
                     (mTLS via PKCS12 identity), Executor::dispatch
  * translate/     — xml_to_json::translate_soap emits
                     {body, headers} with namespaces preserved,
                     attributes as @-keys, repeats as arrays
  * router/        — axum routes + AppState wiring
  * openapi.rs     — build_spec walks ServiceMap, emits stable
                     OpenAPI 3.1 output
  * error.rs       — XtrError with IntoResponse
  * main.rs        — tokio + config load + assemble + serve

Tests (29 pass, 0 fail, 0 ignored)
  * 5 loader (walk, missing path, extensions, non-YAML skip,
    parse error)
  * 6 handlebars (allow-list filter, drop non-allowlist, auto
    context, generate.client shape, generate.uuid validity,
    single-pass regression guard)
  * 8 xml_to_json (body/headers extraction with namespace
    prefixes, UTF-8 Estonian chars, XML entity refs, repeat
    → array, attributes → @-keys, empty → null, malformed
    error, namespaced element names)
  * 5 openapi (empty map, one service, "string" type regression,
    requestBody.required toggle, response schema shape)
  * 5 integration (health, /api lists loaded services,
    end-to-end with mock upstream capturing outbound
    Content-Type + body, unknown-service 404, params filter)

DSL samples
  * DSL/samples/ar/{lihtandmed_v3, detailandmed_v2,
    ettevottegaSeotudIsikud_v1, tegelikudKasusaajad_v2}.yml
  * DSL/samples/xroad/{listMethods, allowedMethods}.yml

  Imported verbatim from buerokratt/XTR. Live smoke test loads
  all six into GET /api as OpenAPI operations.

Docs
  * book/src/dsl/format.md — new. DSL format, params allow-list,
    service field semantics, Handlebars auto-context, response
    shape, end-to-end example.
  * book/src/getting-started/run-locally.md — refreshed with
    real /health + /api output; shipped-sample invocation
    recipe.
  * book/src/getting-started/automated-tests.md — baseline
    updated to 29/0/0.
  * book/src/SUMMARY.md — new DSL section.

### Added — task epic system + follow-ups (earlier this cycle)

- **`docs/DESIGN.md`** — the domain design derived from a direct
  read of the original [buerokratt/XTR](https://github.com/buerokratt/XTR).
  Documents the JVM XTR's public surface, DSL format, config,
  request lifecycle, and 17 known bugs. Defines the XTR-on-Rust
  MVP scope (`v0.1.0-rc.2`), correctness fixes applied, non-goals,
  crate layout, roadmap to v1.0. Now includes **§2.7 X-Road
  protocol context** — the domain gotchas beyond mechanical
  translation, each cross-linked to a follow-up task.
- **`tasks/backlog/002-implement-mvp-v0.1.0-rc.2.md`** — next
  task on the roadmap: implement DESIGN.md §8 (the MVP slice).
- **Task epic system** in `tasks/backlog/epic-*/`. Three epics
  filed after the task 001 review, each with its own README:
  - `epic-xroad-protocol-compliance/` — 3 open tasks (003, 004,
    005: Content-Type, response requestHash verification,
    explicit protocol version in config).
  - `epic-operator-onboarding/` — 1 open task (006: X-Road cert
    acquisition + keystore setup docs).
  - `epic-testing-infrastructure/` — 2 open tasks (007, 008:
    mock X-Road Security Server for CI, UTF-8 / Estonian
    charset round-trip test).
- **Empty `main` branch** — orphan commit with a README
  redirecting to `dev`. Reserved for the future `v1.0.0`.

### Changed

- `HANDOFF.md` — roadmap section rewritten. Task 001 marked
  done; task 002 up next. New "Open backlog" table listing
  top-level tasks + epics.
- `README.md` Status section now surfaces the domain design.
- `book/src/introduction.md` first paragraph points at
  `docs/DESIGN.md`.
- **`STANDARDS.md` §13 extended** — task tracking now allows
  optional epic subdirectories (`tasks/backlog/epic-<slug>/`
  mirrored to `done/` on completion). New "Linking rule"
  clause: every commit must reference at least one task file.

### Task tracking

- Task 001 (deep-dive) moved from `backlog/` to `done/` with a
  Landed note.

## [0.1.0] - 2026-07-28

Initial scaffold. Standards-compliant repo skeleton — no shipped
domain functionality yet. Every rule from Ruuter-on-Rust's
`STANDARDS.md` applied from day one.

### Added

- Rust binary crate (`xtr-on-rust`) — placeholder `main.rs` that
  prints a scaffold notice and exits. MSRV pinned to 1.88.
- **CI workflows** (`.github/workflows/`):
  - `tests.yml` — matrix on `ubuntu-latest` + `ubuntu-24.04-arm`,
    `cargo fmt --check` + `cargo clippy --all-targets -- -D
    warnings` + `cargo test --release --no-fail-fast`.
  - `security.yml` — `cargo audit --deny warnings` + `cargo deny
    check all` on push/PR/daily cron.
  - `publish.yml` — multi-arch (`linux/amd64` + `linux/arm64`)
    Docker Hub + GHCR publish on release tag or
    `workflow_dispatch`. Cosign keyless signing, SPDX SBOM,
    in-toto provenance, Trivy vulnerability scan gates signing,
    smoke test both platforms. Supports SemVer pre-release tags
    with maturity-scoped moving tag (`:rc`, `:beta`, `:alpha`).
  - `docs.yml` — mdBook build + GitHub Pages deploy on push to
    `main`.
- **Supply-chain configs**:
  - `deny.toml` — Apache-2.0-compatible license allow-list, banned
    wildcards, crates.io-only sources.
  - `.cargo/audit.toml` — empty exceptions stub (mirror any
    entries here into `deny.toml`'s `[advisories].ignore`).
- **Hardened container**:
  - `Dockerfile` — multi-stage `rust:1.88-slim` →
    `debian:bookworm-slim`, non-root uid 1000, `tini` as PID 1.
  - `docker-compose.yml` — `read_only: true`, `cap_drop: [ALL]`,
    `no-new-privileges: true`, CPU + memory limits, `HEALTHCHECK`.
- **Documentation scaffold** (mdBook at `book/`):
  - Getting Started chapters: Prerequisites → Run it locally →
    Watch the automated tests pass → What to read next.
  - Ops chapter: Docker (with placeholder cosign verify recipe).
  - Light-on-white theme (`book/theme/custom.css`).
- **STANDARDS.md** — the reference document capturing every rule
  this project inherits. Reusable by any `<Product>-on-Rust`
  sibling.
- **SECURITY.md** — private disclosure recipe, response SLA,
  supported versions, CI supply-chain posture inventory.
- **HANDOFF.md** — entry point for the next contributor.
- **`tasks/backlog/001-domain-deep-dive-original-xtr.md`** —
  first task on the roadmap: analyse the original
  `buerokratt/XTR` and define XTR-on-Rust's domain surface.

[Unreleased]: https://github.com/turnerrainer/XTR/compare/v0.4.0-rc...HEAD
[0.4.0-rc]: https://github.com/turnerrainer/XTR/compare/v0.3.0-rc...v0.4.0-rc
[0.3.0-rc]: https://github.com/turnerrainer/XTR/compare/v0.2.0-rc.1...v0.3.0-rc
[0.2.0-rc.1]: https://github.com/turnerrainer/XTR/compare/v0.2.0-rc...v0.2.0-rc.1
[0.2.0-rc]: https://github.com/turnerrainer/XTR/compare/v0.1.0-rc.2...v0.2.0-rc
[0.1.0-rc.2]: https://github.com/turnerrainer/XTR/compare/v0.1.0-rc.1...v0.1.0-rc.2
[0.1.0-rc.1]: https://github.com/turnerrainer/XTR/compare/v0.1.0...v0.1.0-rc.1
[0.1.0]: https://github.com/turnerrainer/XTR/releases/tag/v0.1.0
