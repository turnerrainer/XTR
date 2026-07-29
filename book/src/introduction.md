# XTR

REST proxy for X-Road SOAP services. Rust reimplementation of
[buerokratt/XTR](https://github.com/buerokratt/XTR).

Point XTR at a folder of WSDL files. It parses each one, materialises
every `wsdl:operation` as a `POST /group/operation` REST endpoint,
and translates SOAP responses to JSON. Ships with **194 live
endpoints** from real Estonian X-Road services (Ariregister + Maa-amet
+ Keskkonnaamet + RMK + Kliimaministeerium) ready to call.

**Version:** 0.1.0-rc.2 · **License:** Apache-2.0
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
{"error":"upstream_soap_fault","message":"upstream returned SOAP Fault (SOAP-ENV:Server): Incorrect user name or password.","code":"SOAP-ENV:Server","string":"Incorrect user name or password.","detail":null}
```

## Read in order

1. [Getting started](./getting-started.md) — install, run, add your own service
2. [Configuration](./configuration.md) — `xtr.yaml` reference
3. [WSDL folder-drop](./wsdl-ingestion.md) — auto-generate DSLs from WSDLs
4. [Security Server setup](./security-server.md) — needed for real X-Road services
5. [Failure modes](./failure-modes.md) — every HTTP status XTR emits
