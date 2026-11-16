# XTR

XTR is a REST-facing proxy for X-Road services, extended to
adjacent government integration lanes (ERRU, eDelivery). Point it
at a folder of service definitions and it publishes each one as an
HTTP endpoint. Callers speak plain HTTP to XTR; XTR speaks mTLS to
the X-Road Security Server on their behalf.

Four lanes share one deployment:

- **SOAP** — auto-generated from WSDL files. Ships with **194 live
  endpoints** for real Estonian X-Road services (Ariregister +
  Maa-amet + Keskkonnaamet + RMK + Kliimaministeerium). SOAP
  responses are translated to JSON.
- **REST** — hand-written DSL, passthrough. Body + headers + query
  forwarded verbatim over mTLS. Implements the
  [X-Road Message Protocol for REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).
- **Schema-aware SOAP (both directions)** — a `<name>.soap.yaml`
  sidecar next to a WSDL turns XTR into a SOAP 1.1 provider
  (`/soap-in/`) and/or a schema-aware JSON → SOAP client
  (`/soap-out/`), sharing one XML ⇄ JSON codec. See
  [Schema-aware SOAP lanes](./soap-lanes.md).
- **Raw-XML (non-SOAP)** — a `<name>.xml.yaml` sidecar next to XSD
  files enables lanes for peers that exchange bare XML over HTTPS,
  no SOAP envelope (e.g. the EU ERRU MOVEHUB). See
  [Raw-XML lanes](./xml-lanes.md).

Rust reimplementation of [buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.5.1-rc · **License:** Apache-2.0
· **Repo:** [turnerrainer/XTR](https://github.com/turnerrainer/XTR)
· **Images:** `docker.io/turnerrainer/xtr:rc`, `ghcr.io/turnerrainer/xtr:rc`

## One-command demo

```bash
docker run -d --name xtr -p 8080:8080 turnerrainer/xtr:rc
curl http://localhost:8080/health          # {"status":"ok"}
curl -s http://localhost:8080/api | jq '.paths | keys | length'   # 194
```

Real call against the real Estonian Business Register (fake creds →
real SOAP fault, which proves the wire round-trip works):

```bash
curl -sX POST http://localhost:8080/ariregister/lihtandmed_v3 \
  -H 'content-type: application/json' \
  -d '{"ariregister_kasutajanimi":"x","ariregister_parool":"x","ariregistri_kood":"70006317","ariregister_sessioon":"","ariregister_valjundi_formaat":"","evnimi":"","evarv":"","keel":""}'
```

Response:

```json
{"error":"upstream_soap_fault","message":"upstream returned SOAP Fault (SOAP-ENV:Server)","code":"SOAP-ENV:Server","string":"Incorrect user name or password."}
```

## Read in order

1. [Getting started](./getting-started.md) — install, run, add a SOAP service, add a REST service
2. [Configuration](./configuration.md) — `xtr.yaml` reference (every field, every default)
3. [HTTP response contract](./http-contract.md) — middleware stack, default headers, error shapes
4. [WSDL folder-drop](./wsdl-ingestion.md) — auto-generate SOAP DSLs from WSDLs
5. [REST passthrough](./rest-passthrough.md) — REST-lane DSL reference (wire protocol, headers, security posture)
6. [Schema-aware SOAP lanes](./soap-lanes.md) — SOAP 1.1 provider + schema-aware client via `.soap.yaml` sidecar
7. [Raw-XML lanes](./xml-lanes.md) — bare XML / HTTPS for non-X-Road peers via `.xml.yaml` sidecar
8. [Security Server setup](./security-server.md) — mTLS keystore + trust CA (required for X-Road routing)
9. [Doctor & migration](./doctor.md) — validate `xtr.yaml` before deploy
10. [Failure modes](./failure-modes.md) — every HTTP status XTR emits
