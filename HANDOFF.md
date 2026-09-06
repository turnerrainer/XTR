# HANDOFF

**Written**: 2026-07-29
**Last touched**: 2026-09-07 — dev now reflects the merged
`0.2.0-rc.1` state (audit-v1 fixes + release gate + Dockerfile
ENTRYPOINT hotfix). See [`MIGRATION.md`](./MIGRATION.md) if
you're upgrading a live deployment from `0.1.0-rc.2`.
**Current published**: `turnerrainer/xtr:0.2.0-rc.1` (and
`ghcr.io/turnerrainer/xtr:0.2.0-rc.1`). Moving tag
`:rc` currently floats to `0.2.0-rc.1`. Older immutable pins
still resolve: `:0.2.0-rc` (pre-hotfix; server works,
`docker run … doctor` recipe broken), `:0.1.0-rc.2`
(digest `sha256:61d441d00f75`).
**Branch**: `dev` — clean; the three release-line PRs
[#2](https://github.com/turnerrainer/XTR/pull/2)
(audit-v1 fixes),
[#3](https://github.com/turnerrainer/XTR/pull/3)
(release gate to `0.2.0-rc`), and
[#4](https://github.com/turnerrainer/XTR/pull/4)
(hotfix `0.2.0-rc.1` for Dockerfile ENTRYPOINT) are all merged.
**Last verified green** (on `dev`, 2026-09-07): `cargo test`
156/0/0; fmt + clippy `-D warnings` clean; cargo audit clean;
cargo deny check clean; `docker run --rm -v
$(pwd)/xtr.yaml:/app/xtr.yaml:ro turnerrainer/xtr:rc doctor
--strict` → 0 FATAL, 0 BREAK, 1 WEAK
(`weak-wsdl-allowlist-empty` — the shipped demo posture; close
it in prod by pinning `wsdl.upstream_host_allowlist` per
[`SECURITY.md`](./SECURITY.md)), 3 INFO, exit 1 (WEAK promoted
by `--strict`).

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
- ✅ h2ck.me audit v1 — C1/C2 + H1-H4 + M1-M3 closed; merged
  via PR #2. Ships `xtr-on-rust doctor` config validator +
  `MIGRATION.md` upgrade guide.
- ✅ Release `0.2.0-rc` — merged via PR #3; multi-arch
  Docker Hub + GHCR publish signed + SBOM + provenance.
- ✅ Hotfix `0.2.0-rc.1` — merged via PR #4. Dockerfile
  `ENTRYPOINT` now pins the binary; `docker run … doctor`
  recipe (documented in `MIGRATION.md` and `book/src/doctor.md`)
  works on `:rc` / `:0.2.0-rc.1`. New contract test
  `tests/dockerfile_entrypoint.rs` guards against regression.

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

---

## h2ck.me security-audit pipeline

**Added**: 2026-09-06. Describes the ongoing pre-publication security audit + fix + review flow with the `h2ckme` private GitHub org. v1 is closed (PR #2 merged); if a `feat/audit-vN-*` PR is open when you land here, start with this section.

### What it is

h2ck.me runs a versioned audit → fix → validate cycle against every Bürostack-fleet service before it goes public. Each round is a `vN/` folder in the corresponding private repo under [`github.com/h2ckme`](https://github.com/h2ckme):

- `vN/AUDIT.md` — findings by severity, file:line pointers, attack scenarios.
- `vN/FIX-KIT.md` — runnable attack sandbox, diff-shaped fix code, per-finding acceptance criteria.
- `vN/PR-REVIEWS/<pr-number>-<head-sha7>.md` — one per PR reviewed.

**Fleet-wide index** — [`h2ckme/security-fleet` → `REVIEW-INDEX.md`](https://github.com/h2ckme/security-fleet/blob/main/REVIEW-INDEX.md).

### Where feedback lives (hybrid pipeline as of 2026-09-06)

1. **The v1 audit PR (now merged) carries a comment** starting with `## h2ck.me v1 review`.
2. **Full per-PR write-up** at [`h2ckme/XTR/v1/PR-REVIEWS/`](https://github.com/h2ckme/XTR/tree/main/v1/PR-REVIEWS).
3. **Audit + fix-kit context**: [`h2ckme/XTR/v1/AUDIT.md`](https://github.com/h2ckme/XTR/blob/main/v1/AUDIT.md) + [`v1/FIX-KIT.md`](https://github.com/h2ckme/XTR/blob/main/v1/FIX-KIT.md).

**h2ckme access**: `git clone git@github.com:h2ckme/XTR.git` (private, read via org membership).

### v1 PR on this repo (merged 2026-09-06)

| PR | Branch | Findings | h2ck.me verdict |
|---|---|---|---|
| [#2](https://github.com/turnerrainer/XTR/pull/2) | `feat/audit-v1-security-fixes` (merged) | C1 WSDL SSRF, C2 XML bomb safety net, H1 schema-include path traversal, H2 sidecar client impersonation, H3 SOAP fault detail leak, H4 TLS defaults, M1-M3 | ⚠️ pass-with-note (verdict is approve; one architectural note re: DNS resolution deferred — see below) |

### Architectural note (⚠️ verdict source)

`src/wsdl/url_guard.rs` deliberately defers hostname → IP resolution to fire time (documented at line 20). Threat closed: literal-IP metadata URLs in a WSDL. Threat still open: attacker-controlled hostname resolving to a metadata IP passes the guard, then reqwest resolves and connects — reaching the metadata endpoint if the operator hasn't pinned `wsdl.upstream_host_allowlist` or configured egress network policy.

**Recommended pre-merge**: add a SECURITY.md paragraph naming the operator recipe (`upstream_host_allowlist` OR container network policy). Optional follow-up: `wsdl.dns_check_at_fire_time: bool` opt-in flag for deployments where per-request DNS cost is acceptable.

### Standout in the fix

`url_guard.rs` uses the `Host::` typed enum (not `host_str()`) — correctly avoids the IPv6-brackets footgun. IPv4-mapped-IPv6 unwrap BEFORE range check. **C1 fallback drops bad URL rather than refusing the whole WSDL** — the right call for a 194-WSDL corpus; one bad URL shouldn't kill 193 operations. C2 depth cap self-improved 512→128 with reasoning "a defensive cap that can itself cause a stack overflow is a self-own"; C2 event budget catches wide-and-flat bombs the depth cap misses.

### Next action for a maintainer landing here

The v1 pipeline is closed on our side: PRs #2, #3, #4 all
merged and `:rc` on both registries points at `0.2.0-rc.1`.
What's left:

1. **Wait** — h2ck.me opens `v2/` as an adversarial re-audit
   ~2 weeks after the `0.2.0-rc` publish (target ~2026-09-20).
2. **When v2 lands**, follow the same pipeline: read
   `h2ckme/XTR/v2/AUDIT.md` + `FIX-KIT.md`; open a
   `feat/audit-v2-*` branch off `dev`; land findings; open PR
   for h2ck.me review; merge on green.
3. **Between now and v2**, safe work: v2 backlog items below
   (extraction to workspace crate, DNS-check-at-fire-time
   opt-in, symlink race verification), the open task epics
   (004/007/008/014/015), or unrelated features.

### v2 backlog (from the review)

`url_guard` extraction to a `buerostack-security` workspace crate; `wsdl.dns_check_at_fire_time` flag decision; `resolve_local_schema` symlink race verification; `validate_meta_identity` matrix with partial config.

### h2ck.me does NOT touch this repo

Explicit boundary: h2ck.me writes only to `h2ckme/*` (private org) + PR comment threads. It never pushes code, opens PRs, or edits files in `turnerrainer/*`.
