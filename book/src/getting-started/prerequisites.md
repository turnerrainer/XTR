# Prerequisites

The Getting Started path needs:

| Tool | Why | Install check |
|---|---|---|
| **Docker** + Docker Compose v2 | Runs XTR as a container | `docker compose version` |
| **curl** | Hits endpoints from the shell | `curl --version` |

Optional for the "watch the tests pass" chapter:

| Tool | Why |
|---|---|
| **Rust toolchain** (1.88+) | Compiles the Rust test suite (`cargo test`). Docker-only path works without it. |

Install Rust via [rustup](https://rustup.rs):
`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.

## For real X-Road use

Nothing above is X-Road-specific — the sample DSLs under `ar/`
(Ariregister) call a plain public HTTPS endpoint, so `curl`
against a locally-running XTR is enough to exercise the full
request pipeline.

If you need the *mTLS-to-Security-Server* path (the `xroad/`
sample DSLs, or your own DSLs against a real X-Road service),
you'll also need:

| Item | Notes |
|---|---|
| An X-Road Security Server you own | See [X-Road Security Server setup](../ops/xroad-security-server.md). Test-tier is fully self-service and costs ~€5–15/month VM + a few hours of setup time. |
| A PKCS12 identity | Exported from your SS after subsystem registration. Referenced by `security_server.keystore_path` in `xtr.yaml`. |
| The keystore password | Passed to XTR via the `XTR_KEYSTORE_PASSWORD` env var — never a config default. |

Next: [Run it locally](./run-locally.md).
