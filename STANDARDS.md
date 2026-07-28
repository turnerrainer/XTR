# Standards for XTR-on-Rust (and any sibling `<Product>-on-Rust` project)

This document captures every generic build / documentation / testing /
publishing rule that hardened around **Ruuter-on-Rust** through v0.7
and v0.8.0-rc.1. XTR-on-Rust is being built to the same bar from
day one instead of retrofitting these rules later. Nothing here is
XTR-specific — anything under "product identity" is the only field
that changes between projects.

If a rule below turns out to be wrong for XTR, this file gets edited
and the change carries the rationale. Blind divergence is a code
smell.

---

## 0. Product identity (the only per-project variables)

| Variable | Value for XTR-on-Rust |
|---|---|
| Product name | `XTR-on-Rust` |
| Cargo crate name | `xtr-on-rust` |
| Binary name | `xtr-on-rust` |
| GitHub repo | `github.com/turnerrainer/XTR` |
| Docker Hub image | `turnerrainer/xtr` |
| GHCR image | `ghcr.io/turnerrainer/xtr` |
| License | Apache-2.0 |
| Book title | `XTR-on-Rust` |
| First stable target | `v1.0.0` on `main` |
| Author | Rainer Türner |
| Namespace on Buerostack | `Buerostack/XTR` |

Everything else in this document is identical across the family.

---

## 1. Repository layout

```
XTR/
├── Cargo.toml
├── Cargo.lock                   # TRACKED (bin crate → reproducible builds)
├── VERSION                      # Plain-text mirror of Cargo.toml version
├── README.md                    # Leads with `docker run …:latest`
├── CHANGELOG.md                 # Keep-a-Changelog format, SemVer 2.0
├── SECURITY.md                  # Private disclosure recipe + SLA
├── HANDOFF.md                   # "Read this first" for next contributor
├── LICENSE
├── NOTICE
├── STANDARDS.md                 # THIS FILE
├── Dockerfile                   # Multi-stage, hardened
├── docker-compose.yml           # Production-hardened
├── .gitignore                   # NEVER ignore Cargo.lock
├── .dockerignore
├── deny.toml                    # cargo-deny config
├── .cargo/audit.toml            # cargo-audit exceptions (dated review)
├── src/                         # Rust source
├── tests/                       # Integration tests
├── book/                        # mdBook documentation
│   ├── book.toml
│   ├── theme/custom.css         # Light-on-white theme
│   └── src/
│       ├── SUMMARY.md
│       ├── introduction.md
│       ├── getting-started/
│       ├── reference/
│       └── ops/
├── tasks/
│   ├── backlog/                 # Sequential task IDs, one file each
│   └── done/                    # Landed tasks with "Landed" note appended
└── .github/workflows/
    ├── tests.yml
    ├── security.yml
    ├── publish.yml
    └── docs.yml
```

---

## 2. Rust conventions

- **MSRV pinned** in `Cargo.toml` (`rust-version = "1.88"`). Bump only
  in a dedicated PR with rationale.
- **`Cargo.lock` is tracked.** Binary crates on public registries need
  reproducible builds. This is the mistake we learned once and don't
  repeat.
- **`edition = "2021"`** minimum.
- **`[lints.clippy]` block in `Cargo.toml`** carries any documented
  allow-list (test-fixture patterns, etc.) with the reason inline.
- **`cargo fmt --check`** is a hard CI gate.
- **`cargo clippy --all-targets -- -D warnings`** is a hard CI gate,
  run under **every feature set** (mutually-exclusive features →
  loop the check).
- **Features**: name them by purpose (`scripting-boa`, not `boa`);
  document exclusivity in Cargo.toml comments.
- **Never** skip hooks (`--no-verify`), never `--no-gpg-sign` unless
  explicitly asked.

---

## 3. Testing tiers

Three independent tiers, each catching a different class of mistake:

| Tier | Tool | Runs in | Catches |
|---|---|---|---|
| Rust unit + integration | `cargo test --no-fail-fast` | seconds | Engine invariants, wire-level tests |
| Static domain check | dedicated `dsl-lint`-style bin (if domain has a spec) | ~100 ms | Config/spec typos, missing constants |
| Scenario / e2e | dedicated `dsl-test`-style bin (if applicable) | seconds | Contract of shipped configs (status, body, state) |

- **Baseline metrics tracked** in `HANDOFF.md` and
  `book/src/getting-started/automated-tests.md`
  (e.g. "222 pass / 0 fail / 3 ignored").
- **Ignored tests** must carry an inline reason.
- Tests written to **BREAK** the fix, not confirm it (audit-cycle
  lesson from CLAUDE.md — see §12).
- **Full call-chain tracing** required when fixing bugs at seams.

---

## 4. Documentation

- **mdBook** at `book/`, deployed to GitHub Pages by `docs.yml`.
- **Getting Started chapters come first** — a first-time reader
  should reach a working local install + green test suite + Postman
  proof BEFORE hitting any reference material. Order:
  1. Prerequisites
  2. Run it locally
  3. Watch the automated tests pass
  4. Try the Postman collection (or equivalent scenario runner)
  5. What to read next
- **Every sample on every page must be RUNNABLE** with real captured
  output. Not made-up.
- **Command / response split**: each runnable example is two blocks —
  a `bash` block for the command (no `$` prefix — copy-clean) and a
  labelled `json` / `http` / `console` block for the response. No
  ` ```console ` blocks that mix both.
- **YAML samples in the book must be pure block style** — no
  flow-style `{ … }` maps, no inline `[ … ]` arrays. Paste-straight
  into a `.yml` file.
- **Light-on-white book theme** modelled on the Apache Arrow docs
  (`book/theme/custom.css`). Registered via
  `additional-css = ["theme/custom.css"]` in `book.toml`.
- **README** leads with `docker run <image>:latest` (or `:rc` while
  pre-1.0). Build-from-source is a second-tier path for hackers.
- **Cosign verify recipe** documented in `book/src/ops/docker.md`.
- **`SECURITY.md`** at the repo root — GitHub surfaces it in the
  Security tab automatically. Contains: private disclosure recipe,
  response SLA, supported versions, CI supply-chain posture, out-of-
  scope list.

---

## 5. Security & supply chain

Every commit passes:

- **`cargo audit --deny warnings`** — vulnerabilities + unmaintained
  crates fail hard. Documented exceptions in `.cargo/audit.toml`
  with rationale + **`Review: YYYY-MM-DD`** date. Unreviewed
  entries are a code smell.
- **`cargo deny check all`** — advisories + bans + licenses +
  sources. Config in `deny.toml`. Advisory exceptions must be
  **mirrored** between `deny.toml` and `.cargo/audit.toml` (they
  don't share the file).
  - **License allow-list**: Apache-2.0-compatible only (MIT, BSD-2,
    BSD-3, ISC, Unicode-3.0, Zlib, MPL-2.0, CC0-1.0, 0BSD). No
    GPL / AGPL / SSPL — incompatible with Apache-2.0 binary.
  - **Ban wildcards** (`foo = "*"`) hard.
  - **Sources**: crates.io only. No git-URL deps.
- **Trivy image scan** in `publish.yml`, gated on HIGH/CRITICAL
  fixed CVEs (`--ignore-unfixed` because we can't patch upstream
  Debian).
- **Cosign keyless signatures** on every published digest via
  Sigstore OIDC. Verify recipe published.
- **SPDX SBOM + in-toto provenance** (`mode=max`) attached to every
  multi-arch manifest.
- **Reproducible image layer timestamps** via
  `SOURCE_DATE_EPOCH` (derived from the tag commit's timestamp) +
  `outputs: type=image,rewrite-timestamp=true`. Bit-for-bit Rust
  binary reproducibility is NOT enforced (separate rabbit hole).

---

## 6. CI workflows (files under `.github/workflows/`)

### `tests.yml`
- Triggers: `push` to `[main, dev]`, `pull_request` to `[main, dev]`.
- Matrix on `runs-on: [ubuntu-latest, ubuntu-24.04-arm]` — native
  arm64 catches arch bugs before publish.
- One job per feature set; cache keyed by arch to avoid pollution.
- Steps: build → static lint → `cargo test --release --no-fail-fast`
  → domain-specific scenario runner (if applicable).

### `security.yml`
- Triggers: `push` + `pull_request` to `[main, dev]`, plus
  **`schedule: cron '0 6 * * *'`** (daily) so a fresh advisory
  against unchanged code still fires.
- Jobs: `audit` (`cargo audit --deny warnings`) and `deny`
  (`EmbarkStudios/cargo-deny-action@v2`, `check all`). Run in
  parallel.

### `publish.yml`
- Triggers:
  - `push` to `tags: v[0-9]+.[0-9]+.[0-9]+` (stable)
  - `push` to `tags: v[0-9]+.[0-9]+.[0-9]+-*` (pre-release)
  - `workflow_dispatch` with `inputs.tag` — **escape hatch** so
    config-only fixes can re-run against an existing tag without
    the delete-tag / retag ceremony.
- `concurrency: group: publish-${{ github.ref }}, cancel-in-progress: false`.
- `permissions: contents: read, packages: write, id-token: write,
  attestations: write`.
- Steps: checkout (with `ref: inputs.tag || github.ref`,
  `fetch-depth: 0`) → compose tags (see §8 for tag routing) →
  compute SOURCE_DATE_EPOCH → QEMU + Buildx → login to GHCR + Docker
  Hub → build+push (multi-arch, `rewrite-timestamp=true`, `provenance:
  mode=max`, `sbom: true`, `cache-to: type=gha,mode=max,ignore-error=true`)
  → Trivy scan (fails on HIGH/CRITICAL fixed) → **smoke test both
  platforms** (see §7) → cosign install → cosign sign both registry
  digests.

### `docs.yml`
- Trigger: `push` to `main` (or `dev` if we want RC docs live).
- Builds mdBook → deploys to GitHub Pages.

---

## 7. Smoke test in `publish.yml`

Every per-arch image is booted under QEMU on the runner and probed
before cosign runs. **A signed image is always a working image.**

**Multi-platform gotcha we already hit**: Docker keys locally-cached
images by digest, not by (digest, platform). Running `docker run
--platform linux/arm64 image@sha256:X` after a `--platform linux/amd64`
iteration fails with `cannot overwrite digest`. The fix, baked into
publish.yml:

```bash
for platform in linux/amd64 linux/arm64; do
  docker image rm -f "${IMAGE}" >/dev/null 2>&1 || true
  docker pull --quiet --platform "${platform}" "${IMAGE}"
  cid="$(docker run -d --rm --platform "${platform}" -p 18080:8080 "${IMAGE}")"
  # wait for /health (up to 90s — arm64-via-QEMU cold boot is slow)
  # curl /health, then a domain-specific endpoint
  docker rm -f "${cid}" >/dev/null
done
```

---

## 8. Container image tag routing

Composed in a shell step in `publish.yml`, driven by whether the tag
has a SemVer pre-release suffix (`-...`).

**Stable release** (`v0.7.1`):
- `<image>:0.7.1` (immutable)
- `<image>:0.7` (moving — latest patch on minor line)
- `<image>:latest` (moving — latest stable ever)

**Pre-release** (`v0.7.1-rc.1`, `v0.8.0-beta.2`, `v0.9.5-alpha.3`):
- `<image>:0.7.1-rc.1` (immutable)
- `<image>:rc` (moving — latest RC on any line)
- Never `:latest`, never `:0.7`. **Casual pullers on `:latest`
  never see a pre-release.**

Suffix extraction: `SUFFIX=${VERSION#*-}; SUFFIX_TYPE=${SUFFIX%%.*}` →
gives `rc` / `beta` / `alpha` / etc. Bash parameter expansion, no jq.

---

## 9. Container hardening

Baked into both `Dockerfile` and `docker-compose.yml`:

- Multi-stage: `rust:1.88-slim` → `debian:bookworm-slim`.
- Runtime deps only: `libssl3`, `ca-certificates`, `curl` (for
  `HEALTHCHECK`), `tini` (PID 1).
- Non-root user (uid 1000): `useradd -m -u 1000 <name> && chown -R
  <name>:<name> /app`, `USER <name>`.
- `docker-compose.yml`:
  - `read_only: true`
  - `tmpfs: [/tmp:size=64M]`
  - `security_opt: [no-new-privileges:true]`
  - `cap_drop: [ALL]`
  - Explicit `deploy.resources.limits` on cpus + memory
  - `HEALTHCHECK` uses `curl -fsS http://localhost:<port>/health`
  - `restart: unless-stopped`
- Mount conventions:
  - Domain config / state directories mounted `:ro`
  - Optional operator config file mounted `:ro`
  - Never mount writable directories in production

---

## 10. Release process

Two-branch model:
- **`dev`** — active development, tags all intermediate `x.y.z` and
  `x.y.z-rc.N` releases from here
- **`main`** — reserved for `v1.0.0` and beyond. Nothing merges here
  until the project is ready to commit to public-API stability
- Feature branches → PR into `dev` (branch protection enforces this
  — **never bypass with direct push** even when the account can)

Version bumps:
- SemVer 2.0. Pre-release suffix `-rc.N`, `-beta.N`, `-alpha.N`,
  `-preview.N` all accepted by the publish workflow
- `Cargo.toml` + `VERSION` + `CHANGELOG.md` bumped in one PR
- CHANGELOG follows Keep-a-Changelog (`[Unreleased]`, `[Version] -
  YYYY-MM-DD`)

Publish trigger:
- **Tag push** (`git tag -a v0.8.0-rc.1 -m "…" && git push origin
  v0.8.0-rc.1`) fires `publish.yml` from the tagged commit
- OR **workflow_dispatch** with `inputs.tag=v0.8.0-rc.1` re-fires
  against an existing tag (uses workflow file from dispatched ref)
- `git tag -s` (signed) preferred but not required — cosign
  signature on the image is what matters

Follow-up:
- GitHub Release created via `gh release create <tag> --prerelease
  --title … --notes …`. Pre-release flag is critical — stops
  GitHub from marking it as "Latest Release"
- Release body includes: pull recipes both registries, cosign verify
  recipe, key CHANGELOG excerpt, "not GA / main reserved for v1.0.0"

---

## 11. Registries

- **Docker Hub** repo (`turnerrainer/<product>`) — must exist before
  first publish. Create empty via UI or `hub-tool`
- **GHCR** package (`ghcr.io/turnerrainer/<product>`) — first-push
  under personal namespace requires the package to be **pre-created
  and linked to the repo with Write role** (Package settings →
  Manage Actions access). The `packages: write` scope alone isn't
  enough for GHCR to auto-create a personal-namespace package
- **Repo GitHub Actions permissions**: Settings → Actions → General
  → Workflow permissions must be set to **"Read and write
  permissions"**. Default of "read" caps every workflow's
  GITHUB_TOKEN scope even if it declares `packages: write`
- Repo secrets required for publish:
  - `DOCKERHUB_USERNAME` = Docker Hub account
  - `DOCKERHUB_TOKEN` = Docker Hub access token with **Read, Write,
    Delete** scope
  - GHCR uses auto-injected `GITHUB_TOKEN` — no secret

---

## 12. Development principles (audit-cycle lessons)

Carried from `CLAUDE.md`. Apply to every fix, not just Ruuter:

1. **Trace the full call chain, not just the function.** Open every
   caller of a changed function and every reader of any state it
   writes. Bugs live at seams.
2. **Grep for siblings.** If you change one of N parallel sites,
   grep and verify EVERY site.
3. **Write tests that try to BREAK the fix, not confirm it.** List
   inputs you didn't consider. New parameter? Test None, 0, negative,
   duplicate, missing.
4. **End-to-end > unit at seams.** For any bug at a seam between
   functions, write the integration test that would have caught it.
5. **Audits find bugs IN the prior round's fixes.** Ship after the
   round that audited and found nothing, not the round that
   introduced the fix.
6. **Read your fix as a stranger.** Read the diff out loud, or read
   just the function signature and ask "what would a wrong caller
   pass here?"

---

## 13. Task tracking

- `tasks/backlog/NNN-slug.md` — sequential ID, kebab-case slug.
  IDs are unique per project (not restarted per epic).
- `tasks/done/NNN-slug.md` — moved on completion, with a
  **`## Landed`** section appended (date + summary of what shipped +
  any deferred parts). The original proposal body is preserved as
  historical record.

### Epics (optional grouping)

Related tasks may be grouped under an epic subdirectory:
`tasks/backlog/epic-<slug>/NNN-<slug>.md`. On completion, the task
moves to `tasks/done/epic-<slug>/NNN-<slug>.md` (same epic subdir
mirrored under `done/`) so historical grouping survives.

Rules:

- **Epic membership is optional.** A task with no natural grouping
  goes straight in `tasks/backlog/NNN-<slug>.md`.
- **Epic subdirs are named `epic-<kebab-case-slug>`.** The
  `epic-` prefix makes them visually distinct from tasks.
- **Task IDs stay globally sequential** — epic 1 tasks might be
  003, 005, 009; epic 2 might be 004, 006. Numbers reflect filing
  order across the whole project, not position within an epic.
- **Epic README (optional).** Each epic subdir may contain a
  `README.md` describing what the epic is for, what "done" looks
  like, and what triggers closing the whole epic.

### HANDOFF.md

- The "Open backlog" table (or equivalent section) lists top-level
  backlog tasks + epics with a one-line summary. Refreshed with
  each release.

### Linking rule

**Every commit must reference at least one task file** — either
one being landed, one being filed, or one whose deliverable is
being iterated on. Commit message states the task path explicitly
(e.g. `follow-up on tasks/done/001-…`, `files
tasks/backlog/epic-<slug>/NNN-…`). This keeps arbitrary work off
the tree.

---

## 14. Communication style (for future collaborators, human or agent)

- Commit messages: type-prefix (`feat`, `fix`, `docs`, `ci`,
  `release`, `security`, `tasks`) + scope (optional, in parens) +
  short imperative. Body carries the WHY, not the WHAT.
- Backticks around symbols in commit titles for readability.
- Squash-merge commit body: rewritten at merge time to read as ONE
  coherent change, not "we did X, then decided Y" narrative.
- PR body includes a Test plan checklist.

---

## 15. What NOT to standardise here

Product-specific things stay in the product's own docs:
- Domain semantics (what XTR *does*)
- Feature roadmap
- Non-goals specific to the product

If a rule doesn't apply to at least two `<Product>-on-Rust` projects,
it doesn't belong in this file.

---

## Change log for this document

- **2026-07-28** — Initial version, extracted from Ruuter-on-Rust
  through v0.7 and v0.8.0-rc.1. Every rule here has been road-tested
  through at least one release cycle.
