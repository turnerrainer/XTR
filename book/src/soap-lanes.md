# Schema-aware SOAP lanes (both directions)

One WSDL, either direction — chosen per WSDL by a sidecar file:

| Direction | Who calls whom | Endpoint |
|---|---|---|
| **inbound** | a SOAP 1.1 client (X-Road Security Server, any WSDL consumer) calls XTR; XTR calls your JSON backend | `POST /soap-in/<group>/<name>` |
| **outbound** | your service POSTs JSON to XTR; XTR calls the SOAP peer | `POST /soap-out/<group>/<name>/<operation>` |

The two prefixes are deliberately different: `/soap-in/` is meant for
the SOAP peer network, `/soap-out/` must never be (see
[Exposure](#exposure)).

Both lanes share one XML ⇄ JSON codec that understands what the
[WSDL folder-drop](./wsdl-ingestion.md) Handlebars lane cannot:
**attributes, repeated elements, `xs:choice`, `extension` base types,
qualified/unqualified namespaces**. That lane keeps working unchanged;
this one is opt-in and does nothing without a sidecar.

> Status: on `dev`, not in a published image yet — build it from
> source (`docker build -t xtr:local .`).

## Try it

[`examples/soap-lanes/`](https://github.com/turnerrainer/XTR/tree/dev/examples/soap-lanes)
publishes a REST/JSON flow as an X-Road SOAP service:

- `wsdl/demo/person.wsdl` — a synthetic X-Road v4 SOAP contract
  (`PersonCheck`: unqualified schema, `<request>`/`<response>`
  wrappers, X-Road headers in the binding);
- `wsdl/demo/person.soap.yaml` — the sidecar;
- `backend.py` — a stand-in for a flow written for the X-Road REST
  protocol: guard on `X-Road-Client`, flat JSON in, result wrapped as
  `{"response": "<JSON string>"}`, errors as 4xx;
- `request.xml` — what a Security Server would send.

Start the backend and XTR (host ports 18580 internal, 18581 peer-facing):

```bash
cd examples/soap-lanes
python3 backend.py &
docker run -d --name xtr-soap-demo -p 18580:8080 -p 18581:8081 \
  --add-host host.docker.internal:host-gateway \
  -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" -v "$PWD/wsdl:/app/wsdl:ro" xtr:local
docker logs xtr-soap-demo 2>&1 | grep -E "registered|listening"
```

```console
INFO xtr_on_rust::inbound: inbound SOAP endpoint registered endpoint=/soap-in/demo/person operations=["PersonCheck"]
INFO xtr_on_rust::inbound: schema-aware outbound registered endpoint=/soap-out/demo/person/<operation> operations=["PersonCheck"] target=http://localhost:8081/soap-in/demo/person client_cert=false
INFO xtr_on_rust: listening on 0.0.0.0:8080
INFO xtr_on_rust: inbound SOAP lane (/soap-in/) listening on 0.0.0.0:8081
```

### Inbound: SOAP in, JSON backend, SOAP out

```bash
curl -s -X POST http://localhost:18581/soap-in/demo/person \
  -H 'content-type: text/xml; charset=utf-8' -H 'SOAPAction: ""' \
  --data-binary @request.xml | xmllint --format -
```

```xml
<?xml version="1.0" encoding="UTF-8"?>
<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/">
  <SOAP-ENV:Header>
    <xrd:client xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SUBSYSTEM">
      <id:xRoadInstance>ee-dev</id:xRoadInstance>
      <id:memberClass>GOV</id:memberClass>
      <id:memberCode>70000001</id:memberCode>
      <id:subsystemCode>consumer</id:subsystemCode>
    </xrd:client>
    <xrd:service xmlns:xrd="http://x-road.eu/xsd/xroad.xsd" xmlns:id="http://x-road.eu/xsd/identifiers" id:objectType="SERVICE">
      <id:xRoadInstance>ee-dev</id:xRoadInstance>
      <id:memberClass>GOV</id:memberClass>
      <id:memberCode>70000000</id:memberCode>
      <id:subsystemCode>demo</id:subsystemCode>
      <id:serviceCode>PersonCheck</id:serviceCode>
      <id:serviceVersion>v1</id:serviceVersion>
    </xrd:service>
    <xrd:id xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">4894e35d-bf0f-44a6-867a-8e51f1daa7e0</xrd:id>
    <xrd:protocolVersion xmlns:xrd="http://x-road.eu/xsd/xroad.xsd">4.0</xrd:protocolVersion>
  </SOAP-ENV:Header>
  <SOAP-ENV:Body>
    <tns:PersonCheckResponse xmlns:tns="http://demo.x-road.eu">
      <request>
        <personCode>10000000001</personCode>
      </request>
      <response>
        <checks>
          <item>
            <date>2026-01-16T09:30:00</date>
            <title>Roadside check</title>
          </item>
          <item>
            <date>2026-01-15T14:00:00</date>
            <title>Café inspection</title>
          </item>
        </checks>
      </response>
    </tns:PersonCheckResponse>
  </SOAP-ENV:Body>
</SOAP-ENV:Envelope>
```

What the backend received (its own log line) — the flow's own flat
shape, plus `X-Road-Client` built from the SOAP header:

```console
200 /demo/person-check x-road-client='ee-dev/GOV/70000001/consumer' body={"personCode": "10000000001"}
```

Without the X-Road header the backend's guard rejects the call; its
4xx becomes a SOAP `Client` fault:

```bash
sed '/<SOAP-ENV:Header>/,/<\/SOAP-ENV:Header>/d' request.xml | \
  curl -s -w '\nHTTP %{http_code}\n' -X POST http://localhost:18581/soap-in/demo/person \
  -H 'content-type: text/xml; charset=utf-8' -H 'SOAPAction: ""' --data-binary @-
```

```console
<?xml version="1.0" encoding="UTF-8"?>
<SOAP-ENV:Envelope xmlns:SOAP-ENV="http://schemas.xmlsoap.org/soap/envelope/"><SOAP-ENV:Body><SOAP-ENV:Fault><faultcode>SOAP-ENV:Client</faultcode><faultstring>backend returned HTTP 403: X-Road-Client header is missing or has invalid format</faultstring></SOAP-ENV:Fault></SOAP-ENV:Body></SOAP-ENV:Envelope>
HTTP 500
```

The published contract, with `soap:address` pointing at XTR:

```bash
curl -s 'http://localhost:18581/soap-in/demo/person?wsdl' | grep 'soap:address'
```

```console
    <soap:address location="http://localhost:18581/soap-in/demo/person"/>
```

The peer-facing port serves nothing else, and the internal port does
not serve `/soap-in/`:

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://localhost:18581/soap-out/demo/person/PersonCheck
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://localhost:18580/soap-in/demo/person
```

```console
404
404
```

### Outbound: JSON in, SOAP to the peer, JSON out

The demo's outbound lane targets the same XTR's `/soap-in/`, so one
call runs the whole loop JSON → `/soap-out/` → SOAP → `/soap-in/` →
backend → back:

```bash
curl -s -X POST http://localhost:18580/soap-out/demo/person/PersonCheck \
  -H 'content-type: application/json' \
  -d '{"request": {"personCode": "10000000001"}}' | python3 -m json.tool
```

```json
{
    "header": {
        "client": {
            "@objectType": "SUBSYSTEM",
            "xRoadInstance": "ee-dev",
            "memberClass": "GOV",
            "memberCode": "70000000",
            "subsystemCode": "demo"
        },
        "service": {
            "@objectType": "SERVICE",
            "xRoadInstance": "ee-dev",
            "memberClass": "GOV",
            "memberCode": "70000000",
            "subsystemCode": "demo",
            "serviceCode": "PersonCheck",
            "serviceVersion": "v1"
        },
        "id": "8d97e5af-a2a5-44b8-9d18-141af15b2038",
        "protocolVersion": "4.0"
    },
    "response": {
        "request": {
            "personCode": "10000000001"
        },
        "response": {
            "checks": {
                "item": [
                    {
                        "date": "2026-01-16T09:30:00",
                        "title": "Roadside check"
                    },
                    {
                        "date": "2026-01-15T14:00:00",
                        "title": "Caf\u00e9 inspection"
                    }
                ]
            }
        }
    }
}
```

`item` is an array because the schema says `maxOccurs="unbounded"` — it
stays an array with a single item too.

Clean up:

```bash
docker rm -f xtr-soap-demo
kill %1
```

## Folder layout

```text
wsdl/<group>/<name>.wsdl         # WSDL 1.1, SOAP 1.1 binding
wsdl/<group>/*.xsd               # local xs:include / xs:import targets (never fetched)
wsdl/<group>/<name>.soap.yaml    # enables the lanes for this WSDL
```

The scanned folder is `inbound.wsdl_dir`, defaulting to
`wsdl_watch_dir` ([configuration](./configuration.md#schema-aware-soap-lanes)).

## Sidecar reference

The demo sidecar (`examples/soap-lanes/wsdl/demo/person.soap.yaml`):

```yaml
dsl: false
inbound:
  operations:
    PersonCheck:
      backend: http://host.docker.internal:18091/demo/person-check
  payload: request
  request_pointer: /request
  response_pointer: /response
  response_wrap:
    request: request
    response: backend
outbound:
  url: http://localhost:8081/soap-in/demo/person
  xroad_service:
    member_class: GOV
    member_code: "70000000"
    subsystem_code: demo
```

| Field | Default | Purpose |
|---|---|---|
| `dsl` | `true` | Also generate legacy Handlebars DSLs from this WSDL. `false` also removes the DSLs generated from it on earlier boots — by their `# source:` header, so operations since removed from the WSDL go too, while other WSDLs' and hand-written files stay. |
| `inbound.backend` | — | Base URL; operation `X` is POSTed to `<backend>/X`. |
| `inbound.operations.<op>.backend` | — | Per-operation backend URL (overrides `backend`). |
| `inbound.payload` | `wrapped` | `wrapped`: `{service, operation, soapAction, header, request}`. `request`: only the request. |
| `inbound.request_pointer` | — | JSON Pointer: send only this part of the decoded request (`/request` unwraps X-Road v4 `<request>`). |
| `inbound.response_pointer` | — | JSON Pointer into the backend reply; a string there is parsed as JSON (Ruuter's `{"response": "<JSON>"}`). |
| `inbound.response_wrap.<child>` | — | Build the output element from parts: `request` (after `request_pointer`) or `backend` (after `response_pointer`). |
| `inbound.echo_soap_header` | `true` | Echo the request's `<Header>` children (X-Road SOAP profile requirement). |
| `inbound.forward_xroad_headers` | `true` | Send the X-Road SOAP header to the backend as `X-Road-Client` / `X-Road-Service` / `X-Road-Id` / `X-Road-UserId` / `X-Road-Issue`. |
| `outbound.url` | WSDL `soap:address` | Peer URL. A `TURVASERVER` placeholder → the `security_server` from `xtr.yaml` (URL + its mTLS identity). |
| `outbound.keystore_path` | — | Own PKCS#12 client certificate for 2-way TLS peers. Direct targets only — rejected at boot when the target is the Security Server (that route uses `security_server.keystore_path`). |
| `outbound.keystore_password_env` | `XTR_OUTBOUND_KEYSTORE_PASSWORD` | Env var holding the keystore password. |
| `outbound.trust_ca_path` | — | PEM/DER CA bundle for the peer's server certificate. Direct targets only — rejected at boot for the Security Server target (use `security_server.trust_ca_path`). |
| `outbound.xroad_service.{member_class,member_code,subsystem_code}` | — | Add the X-Road header: client from `client_data`, `serviceCode` = operation, `serviceVersion` from the WSDL's `<xrd:version>`, `userId` from the caller's `X-Road-UserId`. **Required** when the target is the Security Server. |

Direct outbound URLs pass the same URL guard as WSDL-declared ones
(`wsdl.allow_http_upstream`, `wsdl.upstream_host_allowlist`) — the demo
sets `allow_http_upstream: true` only for its plain-http loop.

## JSON convention

| XML | JSON |
|---|---|
| `<a>text</a>` | `"a": "text"` — always a string, never coerced (`"007"` stays `"007"`) |
| `<a x="1"/>` | `"a": {"@x": "1"}` |
| `<a x="1">t</a>` | `"a": {"@x": "1", "#text": "t"}` |
| element with `maxOccurs > 1` | always an array, even with one item |
| undeclared repeats | array when repeated |
| `xsi:nil="true"` | `null` |

Namespace prefixes are dropped on decode (senders choose prefixes
freely). On encode, child order follows the XSD sequence (including
`extension` base types); keys the schema doesn't know follow in key
order; `null` and `[]` emit nothing. Keys that are not plain XML
NCNames (any `:`, markup characters) are dropped, and so are attribute
keys starting with `xml` in any case (`@xmlns` would rewrite the
namespace). Characters XML 1.0 cannot carry even escaped (C0 controls
other than TAB/LF/CR, U+FFFE, U+FFFF) are replaced with U+FFFD.

## Inbound details

Operation dispatch: the Body's first element QName, cross-checked
against `SOAPAction` when both sides declare one. The backend also gets
`X-Xtr-Inbound-Service`, `X-Xtr-Operation` and the caller's
`traceparent`.

| Backend reply | SOAP response |
|---|---|
| 2xx JSON | output element (after `response_pointer` / `response_wrap`) |
| any 2xx, one-way operation | HTTP 202, empty body |
| `{"fault": {"code": "Client"\|"Server", "string": "…", "detail": {"<Elem>": {…}}}}` | SOAP Fault; `detail` entries named after a schema element are encoded schema-aware |
| 4xx | Fault `Client`, `faultstring` from `message`/`error` (also looked up under `response_pointer`) |
| 5xx | Fault `Server`, `faultstring` = `backend returned HTTP <status>` only — the backend's own message is logged, and added to the fault only with `expose_soap_fault_detail: true` |
| unreachable, timeout, no backend configured | Fault `Server` |

Full status table: [HTTP response contract](./http-contract.md#schema-aware-soap-lanes).

## Exposure

`/soap-in/` has no caller authentication of its own — SOAP peers
(Security Server) cannot send a bearer token. `/soap-out/` makes XTR
act with **its own identity** (client certificate, X-Road client),
like `/:group/:service`. They must not share an audience.

Give the inbound lane its own listener and expose only that port to the
peer network:

```yaml
port: 8080            # internal: /:group/:service, /api, /soap-out/…
inbound:
  port: 8081          # peer-facing: /soap-in/… and /health only
  public_base_url: https://xtr.example.ee
```

XTR **refuses to boot** when the inbound lane would share the main
listener with anything that calls out with XTR's identity — `/soap-out/`
lanes **or DSL endpoints on `/:group/:service`** (hand-written, or
generated from any WSDL without `dsl: false`, including the shipped demo
DSLs) — while `inbound.port` and `XTR_INTER_SERVICE_TOKEN` are both
unset (`fatal-soap-lanes-shared-listener-no-token`). Inbound on the main
listener with nothing else there is `weak-soap-inbound-shared-listener`.

### Trust model

XTR does not authenticate SOAP callers. The `X-Road-Client`,
`X-Road-Service`, `X-Road-UserId`, … headers it sends to the backend
are copied from the **SOAP envelope** — whoever can reach `/soap-in/`
can put anything there. They are trustworthy only because the Security
Server has already authenticated the caller **and is the only thing
that can reach `inbound.port`**. Enforce that: network policy /
firewall, or an ingress that terminates TLS and verifies the Security
Server's client certificate. This is the same trust a REST provider
places in the headers its Security Server sets — the lane adds no new
trust, but it does not remove the need for that boundary. For peers
that are not a Security Server set `forward_xroad_headers: false`.

## Boot validation and doctor

A WSDL **with** a sidecar is validated strictly at boot; any problem
stops XTR with the full list — invalid sidecar, sidecar without a WSDL,
a local `xs:include` / `xs:import` that is missing or unparsable, a type
or element the operations use that no schema defines, two WSDLs mapping
to the same endpoint name, two different XSD files that would be
published under the same `/soap-in/<group>/<file>.xsd`, an unusable
`outbound:`. With a typo in the demo sidecar
(`paylod:` instead of `payload:`):

```bash
docker run --rm -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" -v "$PWD/wsdl:/app/wsdl:ro" xtr:local 2>&1 | grep -A1 '^Error'
```

```console
Error: invalid SOAP lane configuration (run `xtr-on-rust doctor` for details):
  /app/wsdl/demo/person.soap.yaml: invalid sidecar: inbound: unknown field `paylod`, expected one of `backend`, `operations`, `echo_soap_header`, `payload`, `response_pointer`, `request_pointer`, `response_wrap`, `forward_xroad_headers` at line 8 column 3
```

```bash
docker run --rm -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" -v "$PWD/wsdl:/app/wsdl:ro" xtr:local doctor 2>&1 | grep -A2 'soap-sidecar'
```

```console
  • [fatal-soap-sidecar-invalid] A .soap.yaml sidecar or its WSDL is unusable — XTR refuses to boot
    field:    /app/wsdl/demo/person.soap.yaml
    why:      Sidecars are explicit opt-in config: a typo (unknown field,
```

Rules: `fatal-soap-sidecar-invalid`, `fatal-soap-outbound-invalid`,
`fatal-soap-lanes-shared-listener-no-token`,
`weak-soap-inbound-shared-listener`,
`weak-soap-inbound-op-without-backend` — see the
[doctor catalogue](./doctor.md#schema-aware-soap-lanes-soapyaml). The
doctor checks sidecars statically and never opens a keystore. WSDLs
without a sidecar keep the lenient folder-drop behaviour.

## Security posture

- XML parsing rejects `DOCTYPE` (no DTD, no external entities), caps
  nesting depth, honours `limits.max_request_bytes` /
  `max_response_bytes`.
- Backend JSON keys become XML names only if they are plain NCNames;
  `@xml*` attribute keys are dropped; XML-forbidden characters become
  U+FFFD. Char references to such characters (`&#1;`) are rejected on
  input.
- Backend 5xx messages stay out of SOAP faults unless
  `expose_soap_fault_detail: true` (same posture as the outbound lanes).
- `/soap-out/` is gated by `XTR_INTER_SERVICE_TOKEN` like
  `/:group/:service`; `XTR_OFFLINE` short-circuits both lanes.
- X-Road header values are forwarded as UTF-8 bytes; values with
  control characters are dropped, never smuggled into HTTP headers.

## Limits and what is not included

- SOAP 1.1, document/literal only (1.2 → `VersionMismatch` fault).
- **No XSD validation.** The codec follows schema order and
  cardinality but does not enforce facets, enumerations or required
  fields — a backend that omits a required element produces invalid
  XML. Validate in the backend, or in CI with the XSDs.
- No WS-Security, MTOM or attachments. Remote `schemaLocation`s are
  never fetched; ship local copies.
- Only the first SOAP 1.1 binding is used — its portType supplies the
  operations and the service port bound to it supplies the address
  (SOAP 1.2 bindings/ports in the same WSDL are ignored).
- `/soap-in/` and `/soap-out/` routes are not listed in `GET /api`.
- Element namespaces follow each declaration (`form=`,
  `elementFormDefault` of the declaring schema document, chameleon
  includes, `ref=`). Named types are looked up by local name, so two
  same-named types in different namespaces of one WSDL are not
  distinguished.
- `requestHash`, signing and the message log stay with the Security
  Server.
- Peer-specific message profiles on top of SOAP (asynchronous
  callbacks, business-level correlation, acknowledgement workflows) are
  out of scope; 2-way TLS to non-X-Road peers is supported via
  `outbound.keystore_path`.
