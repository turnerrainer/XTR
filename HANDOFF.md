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

---

## h2ck.me security-audit pipeline

**Added**: 2026-09-06. Describes the ongoing pre-publication security audit + fix + review flow with the `h2ckme` private GitHub org. If you land in this repo cold and see an open `feat/audit-v1-*` PR, start here.

### What it is

h2ck.me runs a versioned audit → fix → validate cycle against every Bürostack-fleet service before it goes public. Each round is a `vN/` folder in the corresponding private repo under [`github.com/h2ckme`](https://github.com/h2ckme):

- `vN/AUDIT.md` — findings by severity, file:line pointers, attack scenarios.
- `vN/FIX-KIT.md` — runnable attack sandbox, diff-shaped fix code, per-finding acceptance criteria.
- `vN/PR-REVIEWS/<pr-number>-<head-sha7>.md` — one per PR reviewed.

**Fleet-wide index** — [`h2ckme/security-fleet` → `REVIEW-INDEX.md`](https://github.com/h2ckme/security-fleet/blob/main/REVIEW-INDEX.md).

### Where feedback lives (hybrid pipeline as of 2026-09-06)

1. **The open v1 audit PR carries a comment** starting with `## h2ck.me v1 review`.
2. **Full per-PR write-up** at [`h2ckme/XTR/v1/PR-REVIEWS/`](https://github.com/h2ckme/XTR/tree/main/v1/PR-REVIEWS).
3. **Audit + fix-kit context**: [`h2ckme/XTR/v1/AUDIT.md`](https://github.com/h2ckme/XTR/blob/main/v1/AUDIT.md) + [`v1/FIX-KIT.md`](https://github.com/h2ckme/XTR/blob/main/v1/FIX-KIT.md).

**h2ckme access**: `git clone git@github.com:h2ckme/XTR.git` (private, read via org membership).

### Open v1 PR on this repo

| PR | Branch | Findings | h2ck.me verdict |
|---|---|---|---|
| [#2](https://github.com/turnerrainer/XTR/pull/2) | `feat/audit-v1-security-fixes` | C1 WSDL SSRF, C2 XML bomb safety net, H1 schema-include path traversal, H2 sidecar client impersonation, H3 SOAP fault detail leak, H4 TLS defaults, M1-M3 | ⚠️ pass-with-note (verdict is approve; one architectural note re: DNS resolution deferred — see below) |

### Architectural note (⚠️ verdict source)

`src/wsdl/url_guard.rs` deliberately defers hostname → IP resolution to fire time (documented at line 20). Threat closed: literal-IP metadata URLs in a WSDL. Threat still open: attacker-controlled hostname resolving to a metadata IP passes the guard, then reqwest resolves and connects — reaching the metadata endpoint if the operator hasn't pinned `wsdl.upstream_host_allowlist` or configured egress network policy.

**Recommended pre-merge**: add a SECURITY.md paragraph naming the operator recipe (`upstream_host_allowlist` OR container network policy). Optional follow-up: `wsdl.dns_check_at_fire_time: bool` opt-in flag for deployments where per-request DNS cost is acceptable.

### Standout in the fix

`url_guard.rs` uses the `Host::` typed enum (not `host_str()`) — correctly avoids the IPv6-brackets footgun. IPv4-mapped-IPv6 unwrap BEFORE range check. **C1 fallback drops bad URL rather than refusing the whole WSDL** — the right call for a 194-WSDL corpus; one bad URL shouldn't kill 193 operations. C2 depth cap self-improved 512→128 with reasoning "a defensive cap that can itself cause a stack overflow is a self-own"; C2 event budget catches wide-and-flat bombs the depth cap misses.

### Next action for a maintainer landing here

1. **Open [PR #2](https://github.com/turnerrainer/XTR/pull/2)** and read the `## h2ck.me v1 review` comment.
2. Follow the link for the full acceptance table + break-the-fix probes.
3. Address the SECURITY.md paragraph on the same fix branch (small doc change).
4. **Merge** on your release cadence. XTR moves from 🟡 FIX FIRST → 🟢 SHIP after the merge + doc addition.
5. Bump version + tag + push image.
6. **Wait ~2 weeks**, then h2ck.me opens `v2/` as an adversarial re-audit.

### v2 backlog (from the review)

`url_guard` extraction to a `buerostack-security` workspace crate; `wsdl.dns_check_at_fire_time` flag decision; `resolve_local_schema` symlink race verification; `validate_meta_identity` matrix with partial config.

### h2ck.me does NOT touch this repo

Explicit boundary: h2ck.me writes only to `h2ckme/*` (private org) + PR comment threads. It never pushes code, opens PRs, or edits files in `turnerrainer/*`.
