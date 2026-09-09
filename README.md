# XTR

REST-facing proxy for X-Road services. Rust reimplementation of
[buerokratt/XTR](https://github.com/buerokratt/XTR).

Point XTR at a folder of service definitions. Callers speak plain
HTTP to XTR; XTR speaks mTLS to the X-Road Security Server on their
behalf. Two service kinds share one deployment:

- **SOAP** — auto-generated from WSDL files. Ships with **194 live
  endpoints** for real Estonian X-Road services (Ariregister +
  Maa-amet + Keskkonnaamet + RMK + Kliimaministeerium). SOAP
  responses are translated to JSON.
- **REST** — hand-written DSL, passthrough. Body + headers + query
  forwarded verbatim over mTLS per [X-Road Message Protocol for
  REST v1.0.4](https://github.com/nordic-institute/X-Road/blob/develop/doc/Protocols/pr-rest_x-road_message_protocol_for_rest.md).

**Version:** 0.3.0-rc · **License:** Apache-2.0
· **Docs:** [turnerrainer.github.io/XTR](https://turnerrainer.github.io/XTR/)
· **Images:** `docker.io/turnerrainer/xtr:rc`, `ghcr.io/turnerrainer/xtr:rc`
(the `:rc` tag always floats to the latest release-candidate;
pin to `:0.3.0-rc` for reproducible deploys).

> **Upgrading from `0.2.0-rc.1`?** Existing SOAP DSLs work
> unchanged (no `kind:` field → SOAP). Read [`MIGRATION.md`](./MIGRATION.md)
> for the two externally-visible additions (405 method enforcement
> on non-POST for SOAP DSLs, new `security_server.trust_ca_path`
> field). Run `docker run --rm -v $(pwd)/xtr.yaml:/app/xtr.yaml:ro
> turnerrainer/xtr:rc doctor --strict` — the doctor now includes
> REST-lane rules and prints exactly what to change.

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
  (getting started, config, WSDL folder-drop, REST passthrough,
  Security Server, doctor, failure modes)
- **Migration** — [`MIGRATION.md`](./MIGRATION.md) — upgrade
  guides with doctor recipe, per-change before/after, LLM prompt
  template, CI gate recipe
- **Config validator** — [`book/src/doctor.md`](./book/src/doctor.md) —
  `xtr-on-rust doctor` subcommand recipe + full rule catalogue
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what XTR does and why
- **Security** — [`SECURITY.md`](./SECURITY.md) — reporting, supply-chain
  posture, SSRF operator recipe
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md) — includes
  `[0.3.0-rc]` REST-lane feature entry
- **AI-assistant context** — [`CLAUDE.md`](./CLAUDE.md) — first
  file to read for Claude Code + friends
- **Original JVM XTR** — <https://github.com/buerokratt/XTR>
