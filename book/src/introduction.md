# XTR-on-Rust

Rust re-implementation of [buerokratt/XTR](https://github.com/buerokratt/XTR).
Domain semantics land as the project matures — this scaffold gives
you the standards, the CI pipeline, and the release process from
day one.

**Version:** 0.1.0 (scaffold — no shipped functionality yet) · **License:** Apache-2.0 · **Repository:** [turnerrainer/XTR](https://github.com/turnerrainer/XTR)

## What ships in the scaffold

- Multi-arch container publish workflow (linux/amd64 + linux/arm64)
- Cosign keyless signing, SPDX SBOM, in-toto provenance
- `cargo audit` + `cargo deny` in CI on every push/PR + daily cron
- Trivy vulnerability scan gates publish; smoke test both platforms
- Native arm64 in test matrix
- Hardened Docker image (non-root, `read_only`, `cap_drop: ALL`)
- Same standards as [Ruuter-on-Rust](https://github.com/turnerrainer/Ruuter) —
  see [`STANDARDS.md`](https://github.com/turnerrainer/XTR/blob/dev/STANDARDS.md)
  for the full list

## Read in order

1. [Prerequisites](./getting-started/prerequisites.md)
2. [Run it locally](./getting-started/run-locally.md)
3. [Watch the automated tests pass](./getting-started/automated-tests.md)
4. [What to read next](./getting-started/next-steps.md)

## Audience

Third-party clients: integrators, operators. Not internal
contributors. If you're modifying XTR itself, read the source and
`HANDOFF.md`.
