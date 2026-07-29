# XTR

REST proxy for X-Road SOAP services. Rust reimplementation of
[buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.1.0-rc.2 · **License:** Apache-2.0
· **Docs:** [turnerrainer.github.io/XTR](https://turnerrainer.github.io/XTR/)
· **Images:** `docker.io/turnerrainer/xtr:rc`, `ghcr.io/turnerrainer/xtr:rc`

Point XTR at a folder of WSDL files → 194 live `POST /group/operation`
REST endpoints (Ariregister + Ministry of Climate portfolio) ready
to call on boot.

## One-command demo

```bash
docker run -d --name xtr -p 8080:8080 turnerrainer/xtr:rc
curl -sX POST http://localhost:8080/ariregister/lihtandmed_v3 \
  -H 'content-type: application/json' \
  -d '{"ariregister_kasutajanimi":"x","ariregister_parool":"x","ariregistri_kood":"70006317","ariregister_sessioon":"","ariregister_valjundi_formaat":"","evnimi":"","evarv":"","keel":""}'
```

Real call against the real Estonian Business Register — returns
`upstream_soap_fault: Incorrect user name or password.` for fake creds,
proving the wire round-trip works.

## Build from source

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
docker compose up -d --build
```

## Documentation

- **Book** — [turnerrainer.github.io/XTR](https://turnerrainer.github.io/XTR/)
  (getting started, config, WSDL folder-drop, Security Server, failure modes)
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what XTR does and why
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md)
- **Original JVM XTR** — <https://github.com/buerokratt/XTR>
