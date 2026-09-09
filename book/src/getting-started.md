# Getting started

Install → run → add a SOAP service → add a REST service.
Everything you need to reach a working deployment on one page.

## Prerequisites

Docker + Docker Compose v2. Optional: Rust 1.88+ for source builds
and running the test suite.

## Run

Three ways to start XTR. Any one is enough.

### A. Pre-built image (fastest)

```bash
docker run -d --name xtr -p 8080:8080 turnerrainer/xtr:rc
```

Also available at `ghcr.io/turnerrainer/xtr:rc`. Both are multi-arch
(amd64 + arm64), cosign-signed.

### B. Docker Compose from source

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
docker compose up -d --build
```

First build takes 2–3 minutes; incrementals are seconds.

### C. Cargo (for development)

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
cargo run --release
```

## Verify

```bash
curl http://localhost:8080/health           # {"status":"ok"}
curl -s http://localhost:8080/api | jq '.paths | keys | length'   # 194
```

`/api` returns the auto-generated OpenAPI 3.1 spec — the complete
endpoint list, request/response schemas, and error codes.

## First real call (no Security Server needed)

Ariregister (Estonian Business Register) is the shipped SOAP demo —
no Security Server required, just a vendor username/password. With
fake creds you get a real SOAP fault, which proves the wire works:

```bash
curl -sX POST http://localhost:8080/ariregister/lihtandmed_v3 \
  -H 'content-type: application/json' \
  -d '{"ariregister_kasutajanimi":"x","ariregister_parool":"x","ariregistri_kood":"70006317","ariregister_sessioon":"","ariregister_valjundi_formaat":"","evnimi":"","evarv":"","keel":""}'
```

Every other X-Road service — SOAP or REST — needs a Security
Server. See [Security Server setup](./security-server.md).

## Add your own SOAP service

SOAP DSLs are one YAML file per operation. Two ways to produce
them: auto-generated from a WSDL, or hand-written for services
without a WSDL / when you need custom Handlebars logic.

### From a WSDL (the standard way)

1. Drop the WSDL under `wsdl/<group>/<subsystem>/`:
   ```
   wsdl/my-vendor/my-service/api.wsdl
   ```
2. Create a sidecar `api.meta.yaml` next to it with the X-Road identity:
   ```yaml
   member_class: GOV
   member_code: "70000123"
   subsystem_code: my-service
   ```
3. Restart XTR. Every `wsdl:operation` in the WSDL becomes a
   `POST /my-vendor/my-service-<operation>` endpoint.

Full details: [WSDL folder-drop](./wsdl-ingestion.md).

### Hand-written SOAP DSL (fallback)

1. Create `DSL/<group>/<operation>.yml`:

   ```yaml
   # DSL/example/lookup.yml   →   POST /example/lookup
   params:
     - reg_code
   service: https://example.com/soap        # omit → route via Security Server
   method: POST
   envelope: >
     <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
       <soapenv:Body>
         <lookup>
           <reg_code>{{reg_code}}</reg_code>
         </lookup>
       </soapenv:Body>
     </soapenv:Envelope>
   ```

2. Restart XTR. Available at `POST /example/lookup`.

Field-by-field:

| Field | Purpose |
|---|---|
| `kind: soap` | Optional. Default when omitted — every 0.2.x DSL parses unchanged. |
| `params:` | Allow-list of JSON keys the caller may supply. Anything else is silently dropped before Handlebars sees it — this is your template-injection defense. |
| `service:` | Set to a URL for direct HTTPS. **Omit** to route through the Security Server. |
| `method:` | Almost always `POST`. Non-POST returns `405`. |
| `envelope:` | SOAP envelope as a Handlebars template. |

**Handlebars auto-context** available in every envelope:

| Placeholder | Renders |
|---|---|
| `{{generate.uuid}}` | Fresh UUID per request (X-Road message id) |
| `{{generate.instance}}` | `xroad_instance:` from config |
| `{{{generate.client}}}` | Your `<xroad:client>` element — **triple-brace, always** |
| `{{generate.protocol_version}}` | `xroad_protocol_version:` from config |

Triple-brace `{{{...}}}` disables HTML-escaping — needed anywhere
the value is raw XML. Double-brace `{{...}}` is right for user-
provided text (numeric IDs, names) and provides XML-injection
defense.

## Add your own REST service

REST DSLs declare `kind: rest` and route through the same X-Road
Security Server the SOAP lane uses — over the same mTLS identity.
XTR builds the `/r1/…` URL, sets the mandatory `X-Road-Client`
header, and forwards the caller's body + headers + query verbatim.

1. Create `DSL/<group>/<operation>.yml`:

   ```yaml
   # DSL/rr/isikud.yml   →   GET /rr/isikud
   kind: rest
   method: GET                     # DSL contract — GET only; non-GET → 405
   target:
     member_class: GOV
     member_code: "70008440"
     subsystem_code: rr
     service_code: dde
     # X-Road REST §4.1: versioning lives INSIDE path.
     path: /v1/isikud
   # Optional. Absent → forward every query key unmodified (spec default).
   # Empty [] → drop all. Non-empty → allow-list.
   allowed_query_params:
     - personalCode
   ```

2. Restart XTR. Call it:

   ```bash
   curl -s "http://localhost:8080/rr/isikud?personalCode=38001011234" \
     -H 'X-Road-UserId: EE38001011234'
   ```

XTR forwards to:

```
GET https://<security-server>/r1/ee-test/GOV/70008440/rr/dde/v1/isikud?personalCode=38001011234
X-Road-Client:  ee-test/GOV/70008440/<your-subsystem>
X-Road-Id:      <fresh-uuid>
X-Road-UserId:  EE38001011234
```

Deep dive on the wire behaviour, header semantics, and DSL
options: [REST passthrough](./rest-passthrough.md).

## End-to-end example: both lanes together

A complete `xtr.yaml` + DSL tree serving one SOAP service (via
direct HTTPS to a public vendor) and one REST service (via
Security Server):

**Directory layout:**

```
.
├── xtr.yaml
├── ssl/
│   ├── xtr-client.p12          # your PKCS12 identity
│   └── xroad-ca.pem            # your Security Server's CA bundle
└── DSL/
    ├── ariregister/
    │   └── lihtandmed_v3.yml   # SOAP — direct HTTPS, no SS
    └── rr/
        └── isikud.yml          # REST — through SS
```

**`xtr.yaml`:**

```yaml
dsl_path: ./DSL
port: 8080

xroad_instance: ee-test
xroad_protocol_version: "4.0"

client_data:                            # your X-Road identity
  member_class: GOV                     # (needed for the REST lane's
  member_code: "70000000"               #  X-Road-Client + SOAP lane's
  subsystem_code: my-subsystem          #  <xroad:client> envelope)

security_server:                        # required by REST DSLs +
                                        # any SOAP DSL without `service:`
  url: https://out.test.x-tee.ee:5500/
  keystore_path: ./ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD
  # Almost always needed — real X-Road SS certs are behind an
  # operator-managed private CA that isn't in the system trust store.
  trust_ca_path: ./ssl/xroad-ca.pem
```

**`DSL/ariregister/lihtandmed_v3.yml`** — plain-HTTPS SOAP, no SS:

```yaml
kind: soap                                 # optional; default
service: https://ariregxmlv6.rik.ee/       # direct HTTPS
method: POST
params:
  - ariregister_kasutajanimi
  - ariregister_parool
  - ariregistri_kood
envelope: >
  <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"
                    xmlns:prod="http://arireg.x-road.eu/producer/">
    <soapenv:Body>
      <prod:lihtandmed_v3>
        <prod:keha>
          <prod:ariregister_kasutajanimi>{{ariregister_kasutajanimi}}</prod:ariregister_kasutajanimi>
          <prod:ariregister_parool>{{ariregister_parool}}</prod:ariregister_parool>
          <prod:ariregistri_kood>{{ariregistri_kood}}</prod:ariregistri_kood>
        </prod:keha>
      </prod:lihtandmed_v3>
    </soapenv:Body>
  </soapenv:Envelope>
```

**`DSL/rr/isikud.yml`** — REST passthrough via Security Server:

```yaml
kind: rest
method: GET
target:
  member_class: GOV
  member_code: "70008440"
  subsystem_code: rr
  service_code: dde
  path: /v1/isikud
allowed_query_params:
  - personalCode
```

**Boot and validate:**

```bash
export XTR_KEYSTORE_PASSWORD='<the-p12-password>'

# 1. Doctor first — catches missing SS, wrong URL scheme, empty
#    target fields, identifier charset issues, etc.
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  -v "$(pwd)/DSL:/app/DSL:ro" \
  -v "$(pwd)/ssl:/app/ssl:ro" \
  -e XTR_KEYSTORE_PASSWORD \
  turnerrainer/xtr:rc doctor --strict

# 2. Then boot the server.
docker run -d --name xtr -p 8080:8080 \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  -v "$(pwd)/DSL:/app/DSL:ro" \
  -v "$(pwd)/ssl:/app/ssl:ro" \
  -e XTR_KEYSTORE_PASSWORD \
  turnerrainer/xtr:rc
```

**Call both endpoints:**

```bash
# SOAP — direct HTTPS to Ariregister:
curl -sX POST http://localhost:8080/ariregister/lihtandmed_v3 \
  -H 'content-type: application/json' \
  -d '{"ariregister_kasutajanimi":"x","ariregister_parool":"x","ariregistri_kood":"70006317"}'

# REST — through your Security Server to Population Register:
curl -s "http://localhost:8080/rr/isikud?personalCode=38001011234" \
  -H 'X-Road-UserId: EE38001011234'
```

That's a complete two-lane deployment.

## Collision rules (SOAP DSLs)

If a hand-written DSL and a WSDL-generated DSL would land at the
same path, the hand-written one wins. WSDL-generated files carry a
marker header (`# GENERATED BY XTR from WSDL — do not edit;
delete this line to convert into a hand-written override`).
Deleting that line converts the file into a hand-written override
that will never be overwritten by regeneration.

REST DSLs are hand-written only — no WSDL / OpenAPI generator
today.

## Response shape

**SOAP** — XML `<Body>` and `<Header>` translated to JSON:

```json
{
  "body": { …translated SOAP <Body>… },
  "headers": { …translated SOAP <Header>… }
}
```

XML → JSON translation preserves namespace prefixes as literal
keys (`prod:reg_code`), repeats as arrays, attributes as `@name`
keys, and coerces bare integer / boolean text nodes to typed
values (`42` → `Value::Number`, `"true"` → `Value::Bool`).

**REST** — upstream response bytes returned unchanged, with all
non-hop-by-hop upstream headers (including
`X-Road-Service`, `X-Road-Request-Hash`, `X-Road-Error`) passed to
the caller. No translation, no reshape.

## Response errors

Every XTR-generated error is structured JSON with a stable `error`
code. Full table: [Failure modes](./failure-modes.md).

Upstream errors on the REST lane pass through untranslated — the
caller sees the provider service's own 4xx/5xx status + body.

## Stop

```bash
docker rm -f xtr           # Path A / end-to-end example
docker compose down        # Path B
# Ctrl-C for Path C
```
