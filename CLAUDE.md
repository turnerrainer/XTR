# Context for AI assistants (Claude Code + friends)

Read this first when you land in this repo. Everything below
answers the questions an LLM most often asks in the first 3
turns.

## What this repo is

**XTR-on-Rust** — a REST proxy that fronts X-Road SOAP
services. Point it at a folder of WSDL files, get 194
`POST /group/operation` REST endpoints on boot, translated
to/from JSON.

- **Language**: Rust 1.88 (edition 2021).
- **Framework**: axum + reqwest + quick-xml + handlebars.
- **License**: Apache-2.0.
- **Current version**: `0.2.0-rc.1` (SemVer pre-1.0). Source of
  truth: [`VERSION`](./VERSION) + `Cargo.toml`.
- **Published**: `docker.io/turnerrainer/xtr:rc` and
  `ghcr.io/turnerrainer/xtr:rc` — the `:rc` tag is the moving
  pointer to the latest release-candidate (currently
  `0.2.0-rc.1`). Immutable pins: `:0.2.0-rc.1`, `:0.2.0-rc`,
  `:0.1.0-rc.2` (digest `sha256:61d441d00f75`). Always recommend
  operators pin an immutable tag for prod.

## First files to read

Path | Read when
---|---
[`README.md`](./README.md) | "What is this?" — landing page + demo curl.
[`MIGRATION.md`](./MIGRATION.md) | "How do I upgrade from 0.1.0-rc.2 to 0.2.0-rc(.1)?" — definitive machine + human guide with per-breaking-change before/after, doctor recipe, LLM prompt template, CI-gate snippet.
[`CHANGELOG.md`](./CHANGELOG.md) | "What changed?" — `[0.2.0-rc]` has an explicit "Breaking changes vs 0.1.0-rc.2" subsection; `[0.2.0-rc.1]` documents the Dockerfile ENTRYPOINT hotfix that unblocks the doctor recipe.
[`SECURITY.md`](./SECURITY.md) | "How do I harden it?" — includes the "Operator recipe — SSRF hardening on shared WSDL mounts" section (host allowlist vs egress netpol). Point operators here for the best-practice posture behind `weak-wsdl-allowlist-empty`.
[`docs/DESIGN.md`](./docs/DESIGN.md) | "Why does XTR work the way it does?" — domain design decisions.
[`STANDARDS.md`](./STANDARDS.md) | "What's the coding / build / release ruleset?"
[`book/src/doctor.md`](./book/src/doctor.md) | User-facing recipe + findings model + CI gate for the `doctor` subcommand.
[`book/src/configuration.md`](./book/src/configuration.md) | Annotated `xtr.yaml` reference — every field, every default, every WEAK-vs-strict posture.
[`book/`](./book/src/) | Full mdBook (published at [turnerrainer.github.io/XTR](https://turnerrainer.github.io/XTR/)).

## Common LLM prompts and where they land

### "Help me upgrade this config"

Point the user at `MIGRATION.md` §"For LLM assistants helping
an operator upgrade" — it contains a ready-to-paste prompt
template.

Then have the operator run:

```bash
docker run --rm -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  turnerrainer/xtr:rc doctor --strict
```

Use `:rc` for the latest RC or `:0.2.0-rc.1` for a pinned run.
Do NOT recommend `:0.2.0-rc` for the doctor recipe — that image
predates the ENTRYPOINT hotfix and `docker run … doctor` fails
with `FATAL tini (7) exec doctor failed`. The `:rc` and
`:0.2.0-rc.1` tags carry the fix.

The `doctor` subcommand emits stable-`code` findings you can
map to `MIGRATION.md` §"Doctor rule catalogue" 1:1. JSON shape:
`{severity, code, field, headline, rationale, recovery}` per
finding; pin CI rules to `code`, never to `headline`.

**Gotcha to warn operators about**: an invalid
`xroad_protocol_version` fails at `AppConfig::validate()` before
the doctor pipeline runs. Symptom: bare `Error: internal error:
xroad_protocol_version '<X>' is not one of the accepted values
["4.0", "4.1"]` on stderr, exit 1, empty JSON. Fix the value and
re-run — the doctor's `fatal-config-xroad-protocol-invalid` code
exists but is defensive: `validate()` beats it to the punch on
the real load path.

### "What's the breaking change surface?"

Four items, all documented in `CHANGELOG.md` `[0.2.0-rc]` →
"Breaking changes vs 0.1.0-rc.2" and expanded in
`MIGRATION.md` §"Breaking changes reference":

1. **SOAP fault response shape** — `detail` dropped,
   `faultstring` capped at 200 chars. Recover with
   `expose_soap_fault_detail: true`.
2. **`xroad_protocol_version` enum validation** — must be
   `"4.0"` or `"4.1"`.
3. **Sidecar identity validation** — sidecar
   member_class/code/subsystem_code must match `client_data`.
4. **URL guard drops private-IP WSDL upstreams** — SSRF
   defence; no recovery for literal private IPs.

### "What are the new config fields?"

```yaml
# xtr.yaml
wsdl:
  allow_http_upstream: false          # default; opt-in
  upstream_host_allowlist: []         # optional pinning
expose_soap_fault_detail: false       # default; opt-in
```

Full annotated config in `book/src/configuration.md`; full
rationale in `MIGRATION.md` §"Doctor rule catalogue".

### "How do I run the tests?"

```bash
cargo test                                # 156 tests as of 0.2.0-rc.1
cargo clippy --all-targets -- -D warnings  # style/lint gate
cargo audit --deny warnings                # supply-chain gate
( cd book && mdbook build )                # docs + linkcheck
```

CI mirror in `.github/workflows/{tests,security,docs}.yml`.

### "What best-practice `xtr.yaml` should I recommend?"

Start from the shipped [`xtr.yaml`](./xtr.yaml) (its comments
double as the tour) and layer on:

```yaml
wsdl:
  allow_http_upstream: false               # keep default
  upstream_host_allowlist:                 # pin the SSRF DNS lane
    - ariregxmlv6.rik.ee                   # (adjust to your corpus)
    - jvis.envir.ee
expose_soap_fault_detail: false            # keep default in prod
client_data:                               # fill from RIA before deploy
  member_class: GOV
  member_code: "70000000"
  subsystem_code: "myservice"
```

Then verify: `docker run --rm -v $(pwd)/xtr.yaml:/app/xtr.yaml:ro
turnerrainer/xtr:rc doctor --strict` → expect 0 FATAL, 0 WEAK.
For the alternative "close the DNS lane via container network
policy" posture, see [`SECURITY.md`](./SECURITY.md)
§"Operator recipe — SSRF hardening on shared WSDL mounts".

## Layout at a glance

```
src/
  main.rs           CLI dispatcher (server / doctor subcommands)
  lib.rs            module tree
  config/           AppConfig + serde loader + validate()
  doctor/           doctor.rs — config validator (new in 0.2.0-rc)
  wsdl/
    parser.rs       WSDL SOAP-1.1 parser
    generator.rs    WSDL → DSL YAML
    pipeline.rs     boot-time ingestion + url_guard integration
    url_guard.rs    SSRF guard (new in 0.2.0-rc)
  dsl/              DSL loader + handlebars expansion
  executor/         plain HTTPS + Security Server mTLS clients
  translate/        SOAP XML → JSON
  router/           axum routes
  error.rs          XtrError enum + IntoResponse (H3 shape lives here)
tests/
  it_end_to_end.rs           e2e router + executor
  doctor_integration.rs      subprocess against real binary
  tls_defaults_enforced.rs   H4 self-signed cert integration
```

## Don't

- **Don't add `.danger_accept_invalid_certs(true)`** anywhere.
  There's a regression test (`tests/tls_defaults_enforced.rs`)
  that will flip red.
- **Don't disable `expose_soap_fault_detail` guards** unless
  documenting why in the same PR. That's the H3 audit-v1 fix.
- **Don't bypass `url_guard.rs`** validation on WSDL upstream
  URLs. Its threat model is documented at the top of the file.
- **Don't remove the doctor `code` field or change its
  format** — those codes are the public API for CI pipelines
  pinning to specific findings.
- **Don't write CLAUDE.md-style planning docs** as a side
  effect of a task. Only add docs the user explicitly asks
  for.

## Recent history worth knowing

- **2026-09-07**: `dev` reflects the merged state — PRs #2
  (audit-v1 fixes), #3 (release gate to `0.2.0-rc`), and #4
  (hotfix `0.2.0-rc.1` for Dockerfile ENTRYPOINT). `:rc` on
  Docker Hub + GHCR floats to `0.2.0-rc.1`.
- **2026-09-06**: `0.2.0-rc` cut. Closes h2ck.me audit-v1;
  ships `xtr-on-rust doctor` + `MIGRATION.md`. Same-day hotfix
  `0.2.0-rc.1` fixed a Dockerfile ENTRYPOINT/CMD interaction
  that broke `docker run … doctor` — the recipe now works only
  on `:0.2.0-rc.1` / `:rc`, not on the frozen `:0.2.0-rc` tag.
  See [`CHANGELOG.md`](./CHANGELOG.md) `[0.2.0-rc.1]` for the postmortem.
- **2026-07-29**: `0.1.0-rc.2` published on both registries.

## Sister repos

- `h2ckme/XTR` (private) — pre-publication security audits
  by h2ck.me. `v1/AUDIT.md`, `v1/FIX-KIT.md`, and
  `v1/PR-REVIEWS/*.md` are the paper trail for the current
  audit-v1 fix branch.
- `buerokratt/XTR` — the JVM/Spring predecessor XTR was
  reimplemented from. Refer to it when unsure why an odd
  design choice exists.
