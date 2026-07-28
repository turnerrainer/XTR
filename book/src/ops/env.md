# Environment variables

Every runtime override XTR looks at.

| Variable | Purpose | Read at |
|---|---|---|
| `XTR_CONFIG` | Path to `xtr.yaml` (bypasses cwd search). | Boot. |
| `RUST_LOG` | `tracing_subscriber` filter (`info`, `debug`, `xtr_on_rust=trace`, ...). | Boot. |
| `XTR_KEYSTORE_PASSWORD` | Password for the PKCS12 identity in `security_server.keystore_path`. Env-var name is *configurable* via `security_server.keystore_password_env`; this is the default. **Required if `security_server` is configured — no default value.** | Boot. |

## Container recipe

```bash
docker run -d --name xtr -p 8080:8080 \
    -v $PWD/DSL:/app/DSL:ro \
    -v $PWD/xtr.yaml:/app/xtr.yaml:ro \
    -v $PWD/ssl:/app/ssl:ro \
    -e RUST_LOG=info \
    -e XTR_KEYSTORE_PASSWORD='<from-your-secret-manager>' \
    turnerrainer/xtr:0.2.0-rc.1
```

## What NOT to put in env

- The keystore itself (`.p12`) — mount as a file, not base64-in-env.
- Long-lived credentials in shell history — use a secret manager
  (Vault, GCP Secret Manager, K8s Secret).
- Arbitrary DSL content — DSLs are files.

## See also

- [Configuration](./configuration.md) — the full `xtr.yaml` reference.
- [X-Road Security Server setup](./xroad-security-server.md) — where
  the `.p12` comes from.
