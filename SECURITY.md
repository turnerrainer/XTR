# Security policy

Upgrading from `0.1.0-rc.2` to `0.2.0-rc`? Read
[`MIGRATION.md`](./MIGRATION.md) first — it lists the four
behaviour changes and the concrete recovery flags. Then run
`xtr-on-rust doctor` against your `xtr.yaml` to catch
placeholder credentials, weak URL-guard postures, or
security-server misconfig before you deploy.

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

## Operator recipe — SSRF hardening on shared WSDL mounts

The [audit-v1 C1](./CHANGELOG.md) URL guard rejects literal-IP
metadata endpoints (`169.254.169.254`, RFC-1918 ranges, IPv6
link-local, IPv4-mapped-IPv6, etc.) at WSDL ingest time. It
**does not** resolve hostnames — a WSDL that names
`metadata.attacker.example` and DNS-resolves it to
`169.254.169.254` at request time will pass the guard, and
reqwest will then connect. This is deliberate: DNS at boot
gives a stale-cache false confidence, and per-request DNS
enforcement raises latency for every call.

Close the hostname-DNS lane with one of these two operator
recipes (pick either; both together for high-value
deployments):

1. **Host allowlist in `xtr.yaml`** — pin the set of upstreams:
   ```yaml
   wsdl:
     upstream_host_allowlist:
       - ariregxmlv6.rik.ee
       - jvis.envir.ee
   ```
   Any WSDL or sidecar that names a host outside this set fails
   at boot; DNS trickery becomes irrelevant because unknown
   hostnames never reach the resolver.

2. **Container egress network policy** — deny outbound to
   metadata/loopback/private ranges at the network layer.
   Example for the shipped `docker-compose.yml`:
   ```yaml
   # docker-compose.override.yml
   services:
     xtr:
       # Deny the AWS/GCP metadata IP outright.
       # Add equivalent rules for Azure (169.254.169.254 too),
       # AliCloud (100.100.100.200), and your VPC-private ranges.
       cap_add:
         - NET_ADMIN
       command:
         - sh
         - -c
         - |
           iptables -A OUTPUT -d 169.254.169.254 -j REJECT &&
           iptables -A OUTPUT -d 100.100.100.200 -j REJECT &&
           exec /usr/bin/tini -- /app/xtr-on-rust
   ```
   Or, at the Kubernetes layer, a `NetworkPolicy` egress rule
   with `ipBlock.except` covering all the metadata IPs.

If neither is applied, treat the WSDL mount as a trust boundary
equivalent to code review: only load WSDLs from sources you
would accept commits from.

## Operator recipe — bearer-gate `/:group/:service` on standalone deployments

XTR ships zero built-in caller authentication on
`/:group/:service` — the class-level property surfaced by the
doctor as `info-no-caller-auth`. When XTR sits behind Ruuter,
Ruuter is the intended gate; on Buerostack deployments this is
the standard layout and no additional configuration is needed.

When XTR is deployed **standalone** — bound directly to a
public interface, or on any network segment that carries
untrusted traffic — turn on the bearer-token gate from
[h2ck.me T-8](https://github.com/h2ckme/XTR/blob/main/v1/NEXT-TASKS.md):

```bash
# Generate a 32-byte (256-bit) token from /dev/urandom
export XTR_INTER_SERVICE_TOKEN=$(openssl rand -hex 32)
# Ship the same value to every caller so they can present it.
```

With the env var set at XTR boot:

- Every request to `/:group/:service` requires
  `Authorization: Bearer <TOKEN>`. Missing / wrong / not
  bearer-shaped → HTTP **401** with a structured
  `{"error": "unauthorized", ...}` body.
- `/health` (liveness) and `/api` (OpenAPI spec, separately
  gated by `observability.expose_openapi`) are **never**
  bearer-gated — orchestrators poll `/health` without an
  auth configuration.
- Token equality uses `subtle::ConstantTimeEq` so a timing
  side-channel can't leak the correct prefix byte-by-byte.
  Length mismatches bail early (length is not a security-
  critical secret; an attacker can just guess-and-check each
  length independently).
- The doctor reports the posture: `info-inter-service-token-
  active` (≥ 32 bytes), `weak-inter-service-token-short`
  (< 32 bytes, still enforced but under-entropy), or
  `info-inter-service-token-off` (unset — expected behind
  Ruuter, an operator error on public deployments).

**Rotate** the token by generating a new one, updating callers
first, then updating XTR — a small window of dual-acceptance is
not needed if callers can be updated atomically. For a longer
rotation with dual-acceptance, roll a Ruuter (or reverse-proxy)
in front and swap tokens at that layer instead of at XTR.

Combine with `observability.expose_openapi: false` on public
deployments so `/api` doesn't advertise the DSL surface to
anyone who can reach the port.
