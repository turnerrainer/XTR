# XTR

REST proxy for X-Road SOAP services. Rust reimplementation of
[buerokratt/XTR](https://github.com/buerokratt/XTR).

**Version:** 0.2.0-rc.1 · **License:** Apache-2.0
· **Docs:** [turnerrainer.github.io/XTR](https://turnerrainer.github.io/XTR/)
· **Images:** `docker.io/turnerrainer/xtr:rc`, `ghcr.io/turnerrainer/xtr:rc`
(the `:rc` tag always floats to the latest release-candidate;
pin to `:0.2.0-rc.1` for reproducible deploys).

> **Upgrading from `0.1.0-rc.2`?** Read [`MIGRATION.md`](./MIGRATION.md)
> and run `docker run --rm -v $(pwd)/xtr.yaml:/app/xtr.yaml:ro
> turnerrainer/xtr:rc doctor --strict` — the doctor
> subcommand prints exactly what to change in your config
> to keep behaviour equivalent and where the stronger
> hardening postures live.

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
- **Migration** — [`MIGRATION.md`](./MIGRATION.md) — `0.1.0-rc.2` → `0.2.0-rc`
  upgrade guide with doctor recipe, per-breaking-change before/after,
  LLM prompt template, CI gate recipe
- **Config validator** — [`book/src/doctor.md`](./book/src/doctor.md) —
  `xtr-on-rust doctor` subcommand recipe
- **Design** — [`docs/DESIGN.md`](./docs/DESIGN.md) — what XTR does and why
- **Security** — [`SECURITY.md`](./SECURITY.md) — reporting, supply-chain
  posture, SSRF operator recipe
- **Standards** — [`STANDARDS.md`](./STANDARDS.md) — every generic
  build/docs/test/publish rule the project meets
- **Changelog** — [`CHANGELOG.md`](./CHANGELOG.md) — includes
  `[0.2.0-rc]` breaking-changes subsection
- **AI-assistant context** — [`CLAUDE.md`](./CLAUDE.md) — first
  file to read for Claude Code + friends
- **Original JVM XTR** — <https://github.com/buerokratt/XTR>
