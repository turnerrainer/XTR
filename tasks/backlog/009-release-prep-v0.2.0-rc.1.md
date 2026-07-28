# 009 — Release prep for v0.2.0-rc.1

## Filed

2026-07-28 — follows task 002 (which shipped the MVP domain
slice). Next actionable on the roadmap.

## Scope

Metadata-only release-prep PR that flips XTR-on-Rust from
`0.1.0` (scaffold placeholder) to `0.2.0-rc.1` (first
publishable release candidate).

Same shape as Ruuter-on-Rust's release-prep PR — see the git
history of `github.com/turnerrainer/Ruuter` around v0.8.0-rc.1
for the reference pattern.

## Deliverables

- `Cargo.toml` — bump `version = "0.2.0-rc.1"`.
- `Cargo.lock` — regenerated via `cargo check` (only the
  ruuter-on-rust — I mean xtr-on-rust — line changes).
- `VERSION` — bump to `0.2.0-rc.1`.
- `docker-compose.yml` — bump the `image:` tag to
  `xtr-on-rust:0.2.0-rc.1`.
- `README.md` — version stamp + `docker run` recipe → `:0.2.0-rc.1`.
- `book/src/introduction.md` — version stamp.
- `CHANGELOG.md` — add a `[0.2.0-rc.1] - 2026-07-28` header
  wrapping everything currently under `[Unreleased]`.
- `HANDOFF.md` — refresh "Release" line.

## Not in scope

- Any behaviour change (behaviour ships in task 002 which is
  already done; this task is release-metadata only).
- Actual tag push (that's the operator's move after this PR
  merges — see STANDARDS.md §10 for the flow).

## Prerequisites for the tag push itself

Same as Ruuter (documented in HANDOFF.md "Before the first tag
push"):

1. Create `turnerrainer/xtr` on Docker Hub (empty is fine).
2. Repo Actions permissions: Settings → Actions → General →
   Workflow permissions → "Read and write permissions".
3. Repo secrets: `DOCKERHUB_USERNAME` + `DOCKERHUB_TOKEN`
   (Read/Write/Delete scope).
4. After first GHCR push: link the auto-created package to the
   XTR repo with Write role (personal-namespace quirk).

## Acceptance

- All CI green on the release-prep PR
- Squash-merge to `dev`
- `git tag -a v0.2.0-rc.1` from `dev`
- `git push origin v0.2.0-rc.1` fires `publish.yml`
- Multi-arch image lands on both registries; cosign-signed;
  Trivy-scanned; smoke-tested

## Estimated effort

30 minutes end-to-end (metadata + PR review), plus operator time
for the one-time repo setup.
