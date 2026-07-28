# Docker

XTR-on-Rust ships as a multi-arch (`linux/amd64` + `linux/arm64`)
container image, published on **release tag** push
(`v<major>.<minor>.<patch>`) by the
[publish workflow](https://github.com/turnerrainer/XTR/blob/dev/.github/workflows/publish.yml).

**Note**: no image has been published yet (scaffold, v0.1.0). This
page documents the shape once the first release cuts.

## Where to pull from (once published)

| Registry | Repository | Recommended for |
|---|---|---|
| **Docker Hub** | `turnerrainer/xtr` | Discoverability, casual pulls |
| **GHCR** | `ghcr.io/turnerrainer/xtr` | High-volume / anonymous pulls (no rate limit) |

Both registries carry identical digests. Same tag conventions as
Ruuter-on-Rust — see [STANDARDS.md §8](https://github.com/turnerrainer/XTR/blob/dev/STANDARDS.md#8-container-image-tag-routing).

## Verify the image (cosign, once published)

Every published digest is signed keyless via Sigstore OIDC by the
publish workflow.

Request:

```bash
cosign verify turnerrainer/xtr:latest \
    --certificate-identity-regexp \
      "^https://github.com/turnerrainer/XTR/\.github/workflows/publish\.yml@refs/(tags|heads)/.*$" \
    --certificate-oidc-issuer "https://token.actions.githubusercontent.com"
```

## Container hardening

Baked into both `Dockerfile` and `docker-compose.yml`:

- Multi-stage: `rust:1.88-slim` → `debian:bookworm-slim`
- Runtime deps only: `libssl3`, `ca-certificates`, `curl` (for
  the healthcheck), `tini` (PID 1)
- Non-root user (uid 1000)
- `read_only: true` container FS
- `cap_drop: [ALL]`
- `security_opt: [no-new-privileges:true]`
- CPU + memory limits
- `HEALTHCHECK` against `/health` every 30s

See [STANDARDS.md §9](https://github.com/turnerrainer/XTR/blob/dev/STANDARDS.md#9-container-hardening).
