# Security policy

## Reporting a vulnerability

Please **do not open a public GitHub issue** for security-sensitive
findings. Instead:

1. **Preferred**: use GitHub's private vulnerability reporting for
   this repo — Security tab → **Report a vulnerability**. That
   routes the report to maintainers via a private thread with
   tracking.
2. **Fallback**: email `rainer.turner@gmail.com` with `[XTR-security]`
   in the subject line.

Include, when you can:

- Affected version (image tag or git ref)
- Reproduction steps or PoC
- Impact assessment (what an attacker gains)
- Any suggested mitigation

## Response commitments

- **Acknowledgement**: within 3 business days of the report reaching
  a maintainer.
- **Triage decision** (accepted / needs-more-info / not-a-vuln):
  within 7 business days.
- **Fix + coordinated disclosure**: target 30 days for CRITICAL and
  HIGH severity, 90 days for MEDIUM. Extension is negotiable if a
  fix requires a coordinated upstream change.
- **Credit**: reporters are credited in the release notes unless
  they ask to remain anonymous.

## Supported versions

Only the latest published release receives security fixes. XTR-on-Rust
is pre-1.0 and follows SemVer — minor bumps are the norm, patch
releases are cut only for critical fixes on the current line.

| Version   | Support status                                     |
|-----------|----------------------------------------------------|
| `0.1.x`   | ✅ Supported (current scaffold; no domain yet)     |
| `< 0.1.0` | n/a                                                |

## What we do to reduce supply-chain risk

Every rule below is documented in [`STANDARDS.md`](./STANDARDS.md).

- **`cargo audit --deny warnings`** — every push, every PR, daily at
  06:00 UTC. Advisory exceptions live in `.cargo/audit.toml` with a
  rationale and a review date; blind ignores are a code smell.
- **`cargo deny check all`** — enforces license allow-list
  (Apache-2.0 compatible only, no GPL/AGPL/SSPL), refuses git-URL
  deps and wildcard version specs, warns on duplicate crate
  versions. Config: [`deny.toml`](./deny.toml).
- **Trivy image scan** on every release-tag publish, gated on
  `HIGH` and `CRITICAL` fixed vulnerabilities. Blocks signing.
- **cosign keyless signatures** on every published image digest
  via Sigstore OIDC. Verify recipe in
  [`book/src/ops/docker.md`](./book/src/ops/docker.md#verify-the-image-cosign-once-published).
- **In-toto provenance + SPDX SBOM** attached to every multi-arch
  manifest.
- **Reproducible image layer timestamps** (`SOURCE_DATE_EPOCH` +
  `rewrite-timestamp=true`) so the same commit produces the same
  image digest. Rust binary bit-for-bit determinism is NOT yet
  enforced.
- **Multi-arch smoke test** — every release image is booted under
  QEMU on both `linux/amd64` and `linux/arm64` and probed with
  `/health` before it's signed. A signed image is a working image.
- **Non-root container user** (uid 1000), read-only rootfs,
  `cap_drop: ALL`, `no-new-privileges: true` in the shipped
  `docker-compose.yml`.

## What is out of scope

Domain-specific scope will be documented once XTR's semantics are
defined. As a general rule, the operator is responsible for:

- Secret fetching (Vault / KMS / Docker secrets)
- Persistent state / cross-replica coordination
- Rate limiting (terminate at a reverse proxy)
- IAM / JWT validation at the boundary
