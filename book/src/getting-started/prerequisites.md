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

Next: [Run it locally](./run-locally.md).
