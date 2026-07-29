# HANDOFF

**Written**: 2026-07-29
**Last verified green**: 2026-07-29 — cargo test 84/0/0
(73 unit + 11 integration); fmt + clippy -D warnings clean;
cargo audit clean (0 advisories); cargo deny check clean;
mdbook + linkcheck build clean. Container image
`turnerrainer/xtr:0.1.0-rc.2` (== `:rc`) live on Docker Hub +
GHCR, multi-arch, cosign-signed. `docker pull` from a fresh
machine → 194 endpoints in ~1 s.
**Branch**: `dev` — released as `v0.1.0-rc.2` (tag pushed to
GitHub; publish workflow succeeded 2026-07-29).
**Release**: `v0.1.0-rc.2` published to `docker.io/turnerrainer/xtr`
and `ghcr.io/turnerrainer/xtr` (both `:0.1.0-rc.2` and moving
`:rc` suffix tag).

Next contributor (human or Claude) must:

1. Read [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)
   front-to-back before touching anything. That's the
   authoritative ruleset for all Buerostack Rust projects.
2. Read this file for XTR-specific state.
3. Run the verification set (below) — every command exits 0.

## What this repo IS today

Working REST → SOAP → X-Road proxy in Rust, published as
multi-arch signed container.

- `POST /:group/:service` — DSL lookup → Handlebars expand →
  executor (plain HTTPS or mTLS to X-Road Security Server) →
  XML → JSON translate → response
- `GET /health`, `GET /api` (auto-generated OpenAPI 3.1)
- **WSDL folder-drop** (task 013) — `wsdl_watch_dir:` config
  field. XTR ingests `wsdl/<owner>/<subsystem>/*.wsdl` at boot,
  parses each with in-tree SOAP-1.1 parser, generates
  `DSL/<owner>/[subsystem-]<op>.yml` per operation.
- **194 endpoints shipped** (33 Ariregister + 89 Maa-amet + 38
  Keskkonnaamet + 26 RMK + 6 Kliimaministeerium + 2
  hand-written xroad meta-services). All auto-generated from
  vendored WSDLs under `wsdl/`; DSLs are also committed for
  review.
- **`scripts/harvest-xtee-wsdls.sh`** — fetches ANY public
  Estonian X-Road WSDL from RIA's catalog. Supports
  `--member` and `--subsystem` filters.
- **17 JVM XTR bugs fixed** per DESIGN.md §7.

## Verification set (all should exit 0)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release --bin xtr-on-rust
cargo test --no-fail-fast
cargo audit --deny warnings
( cd book && mdbook build )
```

Live smoke:

```bash
docker run -d --name xtr -p 8080:8080 turnerrainer/xtr:rc
curl http://localhost:8080/health
curl -s http://localhost:8080/api | jq '.paths | keys | length'   # 194
```

## Roadmap

Landed (see [CHANGELOG.md](./CHANGELOG.md) for detail):

- ✅ Task 001 — domain deep-dive
- ✅ Task 002 — MVP per DESIGN.md §8
- ✅ Task 003 — Content-Type + charset
- ✅ Task 005 — X-Road protocol version in config
- ✅ Task 006 — Security Server onboarding docs
- ✅ Task 009 — release prep
- ✅ Task 010 — SOAP Fault detection (200 + non-2xx)
- ✅ Task 011 — request/response size caps + timeout
- ✅ Task 012 — opt-in JSON type coercion
- ✅ Task 013 — WSDL folder-drop + auto-generation
- ✅ Security sweep — quick-xml CVE upgrade, XXE guard, nesting cap
- ✅ First publish — v0.1.0-rc.2 on both registries

Open:

| Task | Location | Notes |
|---|---|---|
| 004 | `tasks/backlog/epic-xroad-protocol-compliance/` | Response requestHash verification (needs real SS or task 007 mock) |
| 007 | `tasks/backlog/epic-testing-infrastructure/` | Mock X-Road Security Server for CI |
| 008 | `tasks/backlog/epic-testing-infrastructure/` | Extend UTF-8 / Estonian character test coverage |
| 014 | `tasks/backlog/epic-developer-experience/` | DSL loader scale optimization (for full RIA catalog) |
| 015 | `tasks/backlog/epic-developer-experience/` | Separate catalog repo (`xtr-catalog-ee`) |

## For the next Claude session refactoring another core component

Everything you need is in these three files:

1. **[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md)** — the
   ruleset. Non-negotiable unless a deviation is justified in
   the commit message.
2. **[`./docs/DESIGN.md`](./docs/DESIGN.md)** — reference
   example of what a "domain design" doc looks like (produced by
   task 001).
3. **This XTR repo** — reference implementation. If in doubt
   about how something should be structured, look at how XTR
   does it.

Common questions answered by files in this repo:

| Question | See |
|---|---|
| How do I structure `Cargo.toml`? | `Cargo.toml` |
| How does the multi-stage Dockerfile work? | `Dockerfile` |
| What goes in `docker-compose.yml`? | `docker-compose.yml` |
| What does `.github/workflows/*` look like? | `.github/workflows/` |
| How do I structure a task file? | any file under `tasks/done/` |
| How do I structure the book? | `book/src/` |
| How is CHANGELOG formatted? | `CHANGELOG.md` |
| How do I set up publish to Docker Hub + GHCR? | DEV-REQUIREMENTS §9 |

## Where to look for more detail

| Topic | File |
|---|---|
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (XTR-specific) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Public docs | https://turnerrainer.github.io/XTR/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |
