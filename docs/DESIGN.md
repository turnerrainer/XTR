# XTR-on-Rust — design document

Source of truth for what XTR-on-Rust should do, derived from a
direct read of the original JVM implementation at
[buerokratt/XTR](https://github.com/buerokratt/XTR) (commit at the
time of writing: `main` on 2026-07-28). Produced by task
[`001-domain-deep-dive-original-xtr`](../tasks/backlog/001-domain-deep-dive-original-xtr.md).

**Audience**: whoever writes the first substantive Rust domain PR.

---

## 1. Executive summary

XTR (X-Road Translator) exposes SOAP-based X-Road services as
JSON-over-REST endpoints. It reads YAML "DSL" files — one per
X-Road service — that describe the URL, allowed parameters, target
service URI, HTTP method, and a Handlebars-templated SOAP envelope.
A client POSTs JSON to `/<group>/<service>`, XTR expands the
envelope, calls X-Road (either directly to a service URI or through
the configured Security Server with client mTLS), translates the
SOAP response XML to JSON, and returns it.

The JVM version is Spring Boot 3.3.0 on Java 17, ~500 lines of
domain code. It's marked "alpha" in its own README, has known
correctness bugs (documented in §7 below), and doesn't ship a
health endpoint. **XTR-on-Rust aims for functional parity plus a
short list of correctness fixes** — not a redesign, not a
feature-expansion round.

---

## 2. What the JVM XTR does — anatomy

### 2.1 Public HTTP surface

Two endpoints, both mounted at the servlet root.

| Method + path | Purpose | Auth |
|---|---|---|
| `POST /<group>/<service>` | Invoke a mapped X-Road service | None (network-perimeter assumption) |
| `GET /api` | Auto-generated OpenAPI 3.0 spec of all mapped services | None |

The `POST` handler is a wildcard (`@RequestMapping(path = "**")`)
that parses the URI into two segments (`uriParts[1]`, `uriParts[2]`)
— **hardcoded two-segment depth**.

### 2.2 DSL format

One YAML file per service. Location: `<dslPath>/<group>/<service>.yml`.
Filename stem (before `.y`) becomes `<service>`; parent directory
becomes `<group>`. The URL is `POST /<group>/<service>`.

```yaml
params:
  - reg_code
service: https://ariregxmlv6.rik.ee/
method: POST
envelope: >
  <soapenv:Envelope ...>
    <soapenv:Body>
      <prod:ettevottegaSeotudIsikud_v1>
        <prod:keha>
          <prod:ariregistri_kood>{{reg_code}}</prod:ariregistri_kood>
        </prod:keha>
      </prod:ettevottegaSeotudIsikud_v1>
    </soapenv:Body>
  </soapenv:Envelope>
```

Fields:
- `params: [<string>]` — allow-list of parameter names. Any
  parameter in the request body **not** on this list is silently
  dropped before Handlebars expansion.
- `service: <URI>` — optional. When present, XTR sends the
  envelope directly to this URI (plain HTTPS via `RestClient`,
  no client cert). When absent, XTR routes through the X-Road
  Security Server (with client mTLS).
- `method: GET | POST` — HTTP method for the upstream call.
- `envelope: <XML>` — SOAP envelope template with Handlebars
  placeholders.

### 2.3 Handlebars context

Two categories of substitution are available inside `envelope`:

**User-provided** (from request body, filtered against `params`):
- Any name in `params:` → the string value from the request body

**Auto-provided** (constants + generators):
- `{{generate.uuid}}` — random UUID per request
- `{{generate.instance}}` — X-Road instance name from
  `application.xroad-instance`
- `{{generate.client}}` — a pre-built `<xroad:client>` element
  containing member-class / member-code / subsystem-code from
  `application.client-data.*`

### 2.4 Request lifecycle

1. Client POSTs `application/json` body to `/<group>/<service>`.
2. Controller looks up `services.get(group).get(service)` — throws
   NPE if missing (no null check).
3. Body is deserialised into `Map<String, String>`.
4. Body is filtered against DSL's `params` — keeps only allowed
   keys.
5. Handlebars template is compiled and applied twice (see §7 bug
   #3) with the merged user + auto context.
6. Executor picks path:
   - If DSL's `service` is non-null and non-blank → plain
     `RestClient` call (no mTLS, uses server's default trust)
   - Else → `WebClient` with mTLS from `ssl/keystore.p12`
     (trust manager accepts ALL certs — see §7 bug #6) to
     `application.security-server`
7. Response XML is parsed, `.Body` node is extracted, serialised
   as JSON, returned to client.

On any exception during execution, controller returns `400` with
the exception's `.getCause()` serialised as body.

### 2.5 Configuration surface (`application.yml`)

| Key | Purpose | Example |
|---|---|---|
| `application.dslPath` | Directory scanned for DSL files | `DSL` (dev), `/DSL` (container) |
| `application.security-server` | X-Road Security Server URL | `https://out.test.x-tee.ee:443/` |
| `application.xroad-instance` | X-Road instance name | `ee-test` |
| `application.client-data.member-class` | Client X-Road member class | `GOV` |
| `application.client-data.member-code` | Client organisation registration code | `70006317` |
| `application.client-data.sybsystem-code` *(sic — typo)* | Client subsystem code | `byrokratt` |
| `application.ssl.keystore-password` | PKCS12 keystore password | `123456` (default!) |
| `application.ssl.certification` / `.key` | Unused / placeholder | — |

Keystore file: hardcoded at `ssl/keystore.p12` (Docker container
runs `generate-keystore.sh` at entrypoint).

### 2.6 Deployment shape

- Multi-stage Dockerfile (`eclipse-temurin:17-jdk` for both stages)
- Runs `generate-keystore.sh` as ENTRYPOINT (creates the mTLS
  keystore at start-up if not present)
- `docker-compose.yml` binds host `9020` to container `8080` (Spring
  Boot default). Dockerfile's `EXPOSE 9010` is misleading — port
  triage note in §7 bug #13.
- Runs as non-root user `xtr`
- Mounts `./DSL` read-write (should be `:ro`)
- Attaches to `bykstack` docker network (Buerostack shared network)

### 2.7 X-Road protocol context (things beyond mechanical translation)

The JVM XTR treats X-Road as an opaque HTTP endpoint. In practice
there's a small set of protocol-specific behaviours a REST-to-SOAP
proxy for X-Road should either respect or explicitly opt out of.
Each item below has a follow-up task filed under
`tasks/backlog/epic-*/` — this section is the narrative context,
those tasks are the actionable units.

**Wire-level things the MVP must get right**:

- **Content-Type on the outbound envelope.** X-Road Security
  Servers expect `text/xml; charset=utf-8` (or
  `application/soap+xml; charset=utf-8`). JVM XTR relies on
  Spring's default. Rust `reqwest` won't guess — set explicitly.
  → **Task 003** (`epic-xroad-protocol-compliance/`).

- **Response `<xroad:requestHash>` verification.** Every X-Road
  response echoes a hash of the request headers. Verifying it
  proves the response is genuinely a reply to *our* request and
  wasn't swapped by a compromised path segment. JVM XTR skips
  this. Perimeter-trust assumption keeps it out of the MVP but
  it's worth filing.
  → **Task 004** (`epic-xroad-protocol-compliance/`).

- **X-Road protocol version.** JVM XTR's sample DSLs hardcode
  `<xroad:protocolVersion>4.0</xroad:protocolVersion>` inside
  each envelope. That's a real X-Road wire protocol identifier
  (currently `4.0` for the SOAP-based flavour; X-Road REST is
  a different protocol version entirely). Making the version
  explicit in config (or per-DSL) prevents accidental drift
  when real deployments upgrade.
  → **Task 005** (`epic-xroad-protocol-compliance/`).

**Deployment / ops things**:

- **Certificate environments.** `ee-test` (staging X-Road
  instance) and `ee-prod` (production) require certificates
  issued by different roots. The PKCS12 keystore for one won't
  authenticate against the other. Operators need clear
  instructions on: obtaining a test cert (there's a public
  self-service registration flow), obtaining a production cert
  (regulated), managing rotation, and picking the right
  Security Server URL per environment.
  → **Task 006** (`epic-operator-onboarding/`).

**Testing things**:

- **No real X-Road in CI.** Integration tests must run against
  a mock Security Server that returns fixture SOAP responses.
  `wiremock-rs` or an axum-based fixture server can host the
  fixtures. Every shipped DSL sample gets a corresponding
  fixture + request/response assertion.
  → **Task 007** (`epic-testing-infrastructure/`).

- **UTF-8 / Estonian character fidelity.** Real X-Road payloads
  routinely carry `ä ö ü õ Š Ž` in person names, company names,
  and addresses. XML → JSON translation must round-trip these
  cleanly. `quick-xml` handles this correctly if configured with
  UTF-8, but the test explicitly exercising it is what stops
  a future refactor from silently breaking it.
  → **Task 008** (`epic-testing-infrastructure/`).

None of these gate the v0.2.0-rc.1 MVP itself — the MVP is about
the mechanical translation working — but they all block a claim
of "production-ready for real X-Road integrations". Track them
under the epics linked above.

---

## 3. Dependencies on other Buerostack services

- **X-Road Security Server** — external, per-deployment. Provides
  the mTLS termination + routing to X-Road member services.
- **Ariregister / other X-Road service providers** — external.
- The stubbed `readServiesFromDB` (sic) code hints at intended
  future integration with a **Resql**-backed WSDL registry, but
  the URI is empty and the method is called from a commented-out
  line. Ignore for now.

XTR is a leaf service. Nothing in the Buerostack stack depends
*on* XTR directly today — Ruuter can call XTR endpoints via
`http.post` if a DSL author sets that up, but there's no bespoke
integration.

---

## 4. Non-goals (from reading the code)

- **Not a general REST-to-SOAP proxy.** X-Road-shaped envelopes
  specifically (`<xroad:client>`, `<xroad:service>`,
  `<xroad:protocolVersion>`).
- **Not an X-Road Security Server.** XTR is a client of one.
- **Not a workflow engine.** One inbound request → one outbound
  X-Road call → one response. No fan-out, no aggregation.
- **Not an ETL.** Request/response only. No batching, no
  streaming.
- **Not a general SOAP toolkit.** Envelopes are string-templated,
  not object-built.
- **No authentication on XTR's own endpoints** — assumed to be
  behind a network perimeter (typically Ruuter or a gateway).
- **No response caching.**
- **No rate limiting.**
- **No hot-reload of DSLs** (loaded once at boot).
- **No structured observability** — plain Log4j only.

---

## 5. What's marked "WORK IN PROGRESS" in the JVM code

Two files carry an explicit "WORK IN PROGRESS" doc-comment header
and are not wired into the request path:

- `SOAPQueryGenerator.java` — WSDL-driven auto-generation of
  `XRoadTemplate` YAML files.
- `DynamicWSDLService.java` — WSDL parsing, XSD downloading,
  operation enumeration, custom SOAP envelope builder.

These are proto-implementations of a "give XTR a WSDL URL, get all
services auto-mapped" feature. **Not shipping in XTR-on-Rust MVP.**
Design ports them to a follow-up task once the request path is solid.

---

## 6. Auto-generated OpenAPI

`OpenApiBuilder` walks the loaded DSLs and produces an OpenAPI 3.0
document served at `GET /api`. For each DSL:

- Path: `/<group>/<service>`
- Method: from DSL's `method` field
- POST: request body is an `object` with the DSL's params as
  properties (all typed as `object` — a bug; see §7 #14). No
  response schema beyond a stub `200`.
- GET: params become query parameters.

**Same pattern as Ruuter's `/_/openapi.json`.** Port lesson:
build the OpenAPI as a `serde_json::Value` at boot; serve from
cache.

---

## 7. Known bugs and rough edges in the JVM version

Documented here so XTR-on-Rust doesn't reimplement them.

1. **Typo `sybsystem-code`** in `application.yml` — code reads
   `subsystem-code` via `@Value`. Result: subsystem-code is
   always null. Fix: use `subsystem-code` consistently.
2. **`@Value` on `static` fields** in `HandlebarsHelper` — Spring
   `@Value` doesn't inject into static fields. All four
   auto-context fields (`xroadInstance`, `memberClass`,
   `memberCode`, `subsystemCode`) are always null at runtime.
   Fix: instance fields; inject via constructor.
3. **Handlebars applied twice, first result discarded.**
   `result.apply(localValues); ... result.apply(values);` — the
   first call's return value is thrown away. Auto-context
   substitutions may or may not survive into the final output
   depending on how Handlebars handles missing keys. Fix: merge
   both maps and apply once.
4. **`readServiesFromDB` (sic)** — method name typo, and
   `RESQL_SERVICE_URI = ""` guarantees any invocation hits an
   empty URL. Fix: cut the whole method for MVP; revisit if
   Resql-backed DSL loading becomes a real need.
5. **Envelope prefix templating** in `HandlebarsHelper.generateClientEnvelope`
   — the `String` literal contains `%s` placeholders followed by
   `.formatted(…)`, but the string never actually gets the values
   substituted because `formatted` is called on the return value
   of `.formatted()` again (Java-doc it, but the flow reads odd).
   Combined with #2 (fields are null), the emitted envelope
   contains literal `%s`.
6. **Trust-all TLS** for the Security Server call —
   `X509TrustManager` returns `null` for `getAcceptedIssuers` and
   silently accepts every cert. Fix: use the system trust store;
   allow a configured CA bundle path.
7. **XML→JSON translation loses SOAP headers** — extracts only
   `.Body`. Some X-Road responses put useful data in headers
   (message id, service id echo). Fix: expose both `header` and
   `body` in the response, or make it per-DSL configurable.
8. **URI parsing hardcodes 2 segments** — no support for
   `/group/subgroup/service`. Fix: match on `/:group/:service`
   axum route (or `/{group}/{service}`).
9. **Exception path returns `e.getCause()`** as body — often
   `null`, often a Java class that doesn't serialise cleanly to
   JSON. Fix: structured error response `{"error": "…", "code":
   "…"}` with proper HTTP status codes.
10. **No `/health` endpoint** — Actuator not enabled. Container
    healthcheck impossible.
11. **All params typed `Map<String, String>`** — no support for
    JSON numbers, booleans, or nested objects in request bodies
    even when the envelope could use them. Fix: `serde_json::Value`
    all the way through; stringify at Handlebars time.
12. **`XTRService.java`** — empty class, unused. Cut.
13. **Port confusion**: `Dockerfile EXPOSE 9010`, but Spring Boot
    listens on `8080`, and `compose.yml` maps `9020:8080`. Three
    numbers, none agree with the others. Fix: pick one (`8080`
    internally, per Ruuter convention) and document.
14. **`OpenApiBuilder` schema type** — for each POST body param,
    sets the property's type to `field.getClass().getSimpleName()`
    where `field` is a `String`. So every property has type
    `"String"` — not a valid OpenAPI type. Fix: emit `"string"`
    (or better, use `serde_json::Value` at DSL load time to infer
    real types).
15. **`RequestExecutorService.doRequestTowarsdSS` (sic)** —
    method name typo (`Towarsd` → `Towards`).
16. **Default keystore password `123456`** in the shipped
    config — obviously insecure. Fix: require operator to supply
    via env var or config; refuse to start if unset and mTLS
    routing is in use.
17. **`YamlXRoadTemplate`** extends `XRoadTemplate` with no
    additions or overrides — dead-weight subclass.

---

## 8. XTR-on-Rust — MVP design (target: v0.2.0-rc.1)

Rust functional parity + fixes for §7 items #1, #2, #3, #6, #7
(partial), #8, #9, #10, #13, #14, #15. Deferred to later: #4, #11
(will start `Map<String, String>` for MVP, migrate to
`serde_json::Value` in v0.3), the two WORK-IN-PROGRESS features
in §5, and the DSL-from-DB / DSL-from-WSDL paths.

### 8.1 Crate layout

```
src/
├── main.rs                       # Entry point — assembles config,
│                                 # loader, router, starts axum
├── lib.rs                        # Re-exports for tests
├── config/
│   └── mod.rs                    # AppConfig + load_or_default
├── dsl/
│   ├── mod.rs
│   ├── template.rs               # XRoadTemplate + YAML deserialise
│   ├── loader.rs                 # Walk dslPath, build map
│   └── handlebars.rs             # Wrap `handlebars` crate + auto-context
├── router/
│   └── mod.rs                    # axum router: POST /:group/:service,
│                                 # GET /api, GET /health
├── executor/
│   ├── mod.rs                    # Route to plain-HTTP or mTLS-SS
│   ├── plain_client.rs           # reqwest::Client (no client cert)
│   └── ss_client.rs              # reqwest::Client with PKCS12 identity
├── translate/
│   └── xml_to_json.rs            # quick-xml → serde_json::Value
├── openapi.rs                    # Build OpenAPI 3.1 spec from loaded DSLs
└── error.rs                      # XtrError enum + IntoResponse
```

### 8.2 HTTP surface (identical shape, cleaner errors)

| Method + path | Purpose |
|---|---|
| `POST /:group/:service` | Invoke a mapped service. Body: JSON object of params. Response: `{ "body": <translated>, "headers": <translated>, "raw": "<xml>" }` (see §8.5 for shape rationale) |
| `GET /api` | OpenAPI 3.1 spec, cached at boot |
| `GET /health` | `{"status":"ok"}` — new. Docker HEALTHCHECK relies on it |

**Route pattern**: `/:group/:service` (axum), NOT wildcard. If we
later need deeper nesting (see §7 #8), add explicit routes; keep
the surface predictable.

### 8.3 DSL format (unchanged wire format)

Same YAML shape as JVM XTR. Rust `serde_yaml_ng` deserialisation:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct XRoadTemplate {
    #[serde(default)]
    pub params: Vec<String>,
    pub service: Option<String>,   // None → route via Security Server
    pub method: HttpMethod,        // GET | POST
    pub envelope: String,
}
```

Loader walks `config.dsl_path` recursively (skip non-YAML). For
each file, derive `(group, service)` from the parent directory and
the filename stem. Store in
`HashMap<(String, String), Arc<XRoadTemplate>>`.

**Hot reload deferred** to v0.3 (borrow Ruuter's `ArcSwap` +
`notify` pattern).

### 8.4 Handlebars context (unified into one apply)

```rust
pub fn expand(template: &str, user_params: HashMap<String, String>,
              cfg: &AppConfig) -> Result<String> {
    let mut ctx = HashMap::new();
    // Auto-context
    ctx.insert("generate.uuid".into(),      Uuid::new_v4().to_string());
    ctx.insert("generate.instance".into(),  cfg.xroad_instance.clone());
    ctx.insert("generate.client".into(),    build_client_element(cfg));
    // User-provided (already filtered against DSL's `params`)
    ctx.extend(user_params);

    let hbs = Handlebars::new();
    hbs.render_template(template, &ctx).map_err(Into::into)
}
```

One apply, one merged context — fixes JVM bug #3.

### 8.5 Response shape

JVM XTR extracts only `.Body` from the SOAP response. XTR-on-Rust
returns both, wrapping in a small envelope so downstream consumers
can pick:

```json
{
  "body": { ... translated SOAP body ... },
  "headers": { ... translated SOAP headers ... }
}
```

If a downstream truly wants "just body" (JVM parity), it reads
`response.body`. The extra `headers` field is additive — no break
for the JVM-parity path. Wire encoding via `serde_json::Value`.

### 8.6 Executor

Two backends, selected per-DSL at request time:

- **Plain HTTPS** (`RequestExecutor::plain`): `reqwest::Client`
  with the system trust store. Used when DSL's `service` field
  is set.
- **Security Server mTLS** (`RequestExecutor::ss`):
  `reqwest::Client` built once at boot with
  `reqwest::Identity::from_pkcs12_der` from
  `config.ssl.keystore_path`. Used when DSL's `service` is None.
  Trust store: system default (NOT trust-all — fixes JVM bug #6).

Both use `reqwest` async; the axum handler awaits directly, no
`.block()` shenanigans.

### 8.7 Configuration

Ported to `ruuter.yaml`-style shape via `serde`:

```yaml
# xtr.yaml
dsl_path: /DSL
xroad_instance: ee-test
client_data:
  member_class: GOV
  member_code: "<your-registry-code>"      # operator supplies
  subsystem_code: <your-subsystem>         # operator supplies — see §2.7
                                           # + task 006 (onboarding docs)

security_server:
  url: "https://out.test.x-tee.ee:443/"
  keystore_path: /app/ssl/keystore.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD   # required — no default
```

Loader precedence (matches Ruuter's `AppConfig::load_or_default`):
`--config <path>` > `XTR_CONFIG` env > `./xtr.yaml` > built-in
defaults.

**Keystore password is env-only** — refuses to start if
`security_server.keystore_path` is set but the env var is
unset. Fixes JVM bug #16.

### 8.8 Error handling

`XtrError` enum with `impl IntoResponse`:

- `TemplateNotFound(group, service)` → 404
- `MissingRequiredParam(name)` → 400
- `HandlebarsError(...)` → 500
- `UpstreamHttpError(status, body_snippet)` → 502 (preserving
  status from X-Road; body snippet capped at 1 KiB)
- `UpstreamTimeout` → 504
- `XmlParseError(...)` → 502
- `KeystoreLoadFailed(...)` → 500 (surfaced at boot, not per-req)
- `InternalError(msg)` → 500

Response body always: `{"error": "<enum-variant-name>", "message":
"<human-readable>"}`. Never leaks internal types.

### 8.9 OpenAPI auto-generation

Walk the loaded DSL map at boot; build a `serde_json::Value`
matching OpenAPI 3.1. Cache the value; serve via `GET /api`. Same
pattern as Ruuter's `/_/openapi.json`. Properties in the request
body schema are typed `"string"` (not `"String"` — fixes JVM bug
#14). Response schema is a generic `object` for MVP; per-DSL
response schema is a follow-up.

### 8.10 Observability

- `tracing` + `tracing-subscriber` with `EnvFilter` (RUST_LOG).
- W3C traceparent header adopted / generated per request (borrow
  Ruuter's implementation verbatim).
- OpenTelemetry OTLP export opt-in via env
  (`OTEL_EXPORTER_OTLP_ENDPOINT`) — same as Ruuter.
- `/health` endpoint always available. Slimmed shape:
  `{"status":"ok"}`.

### 8.11 Ports

Server listens on **`0.0.0.0:8080`** internally. `docker-compose.yml`
maps `8080:8080` externally. Fixes JVM bug #13.

### 8.12 Container

Same hardened posture as Ruuter (already in the scaffold):
- Multi-stage `rust:1.88-slim` → `debian:bookworm-slim`
- Non-root uid 1000
- `read_only: true`, `cap_drop: [ALL]`, `no-new-privileges: true`
- `HEALTHCHECK` against `/health`

Adds one thing: the mTLS keystore mount. Compose file gains:

```yaml
volumes:
  - ./ssl/keystore.p12:/app/ssl/keystore.p12:ro
```

The keystore is **not** built into the image — operators bring
their own.

---

## 9. Roadmap beyond MVP

### v0.3 — quality-of-life

- **Hot-reload DSL files** — port Ruuter's `ArcSwap` + `notify`
  pattern. Opt-in via config flag.
- **Richer JSON types** — `serde_json::Value` instead of
  `HashMap<String, String>` through the request path. Numbers,
  booleans, nested objects usable inside Handlebars.
- **Per-DSL response extraction** — configurable XPath /
  jsonpath to unwrap `Body.<op>.result` etc.

### v0.4 — WSDL introspection

- Port `SOAPQueryGenerator` / `DynamicWSDLService` — give XTR a
  WSDL URL, generate the DSL YAML files automatically.
- Emit DSL files to `dsl_path` and hot-reload picks them up.

### v0.5 — cross-DSL composition (maybe)

- Or delegate this to Ruuter: XTR is called via `http.post` from
  Ruuter DSLs that compose multiple X-Road calls. Cleaner
  separation of concerns.

### v1.0 — API stability commitment

- Freeze the DSL YAML format (backward-compat guarantee)
- Freeze the response shape
- Freeze the config keys
- Commit to SemVer 2.0 breakage rules

---

## 10. What v0.2.0-rc.1 must NOT include

To avoid scope creep, the MVP explicitly defers:

- WSDL introspection (v0.4)
- Hot-reload of DSLs (v0.3)
- Per-DSL response extraction rules (v0.3)
- Non-string JSON body types (v0.3)
- Deeper URL nesting than `/:group/:service` (v0.3 if needed)
- Cross-DSL composition (v0.5 — or delegate to Ruuter forever)
- Authentication on XTR's own endpoints (perimeter assumption
  stays; add if we ever run XTR internet-facing)
- Rate limiting (perimeter assumption)
- Response caching (perimeter assumption; if we need it, use a
  sidecar)
- Resql-backed DSL registry (was empty stub in JVM version)

---

## 11. Cross-references

- Original JVM implementation:
  [buerokratt/XTR](https://github.com/buerokratt/XTR)
- Standards this project follows:
  [`../STANDARDS.md`](../STANDARDS.md)
- Contributor entry point:
  [`../HANDOFF.md`](../HANDOFF.md)
- First-cut roadmap task:
  [`../tasks/backlog/001-domain-deep-dive-original-xtr.md`](../tasks/backlog/001-domain-deep-dive-original-xtr.md)
- Ruuter-on-Rust (sibling project — patterns to borrow):
  [turnerrainer/Ruuter](https://github.com/turnerrainer/Ruuter)

---

## 12. Open questions

None that block the MVP. Two worth flagging for the first PR
review, though:

1. **`GET` method support in DSL.** JVM XTR accepts `method: GET`
   and would send an X-Road GET. Are any real DSLs GET-based? The
   two shipped examples (`xroad/*.yml`, `ar/*.yml`) are all POST.
   Suggest: implement GET (cheap), don't test-drive it until we
   have a real user.
2. **Response `raw` field.** Should the response envelope include
   the raw XML string alongside `body` + `headers`? Useful for
   debugging; adds bytes on the wire. Suggest: gate behind an
   opt-in query param `?debug=1`.

---

*Written 2026-07-28 as part of task 001. Owner reviews on domain
questions; standards questions default to [`STANDARDS.md`](../STANDARDS.md).*
