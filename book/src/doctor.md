# Doctor & migration

XTR ships an in-image config validator: `xtr-on-rust doctor`.
Run it against your `xtr.yaml` before every deploy — it flags
what will break at boot, what changed vs the previous minor
version, and where a stronger security posture is available.

The full migration guide lives in
[`MIGRATION.md`](https://github.com/turnerrainer/XTR/blob/dev/MIGRATION.md)
at the repo root (mirrored in this book under
[reference/migration](./reference/migration.md)).

## Recipe

```bash
docker run --rm \
  -v "$(pwd)/xtr.yaml:/app/xtr.yaml:ro" \
  turnerrainer/xtr:rc doctor --strict
```

## Findings model

| Severity | Meaning | Exit code |
|---|---|---|
| **FATAL** | Server will not boot with this config. | `1` |
| **BREAK** | Behaviour changed vs last minor and your config is on the losing side. Set the named recovery flag if you need bit-for-bit equivalence. | `1` |
| **WEAK** | Currently works, but a stronger posture is available. | `0` normally; `1` under `--strict` |
| **INFO** | Positive observations. | `0` |

The `code` field on every finding (e.g.
`weak-wsdl-allowlist-empty`) is stable across the 0.2.x line —
pin your CI rules to those, not to headlines.

## Machine-readable output

```bash
xtr-on-rust doctor --format json | jq '.[] | select(.severity == "FATAL")'
```

## Recommended CI gate

```yaml
# .github/workflows/xtr-config-gate.yml
name: XTR config gate
on:
  pull_request:
    paths: [xtr.yaml, wsdl/**]
jobs:
  doctor:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - run: |
          docker run --rm \
            -v "$PWD/xtr.yaml:/app/xtr.yaml:ro" \
            -v "$PWD/wsdl:/app/wsdl:ro" \
            turnerrainer/xtr:rc doctor --strict --format json \
          | tee doctor.json
      - run: |
          fatal=$(jq '[.[] | select(.severity=="FATAL")] | length' doctor.json)
          [ "$fatal" -eq 0 ] || { echo "::error::$fatal FATAL finding(s)"; exit 1; }
```

See the [migration reference](./reference/migration.md) for the
full rule catalogue and the exact breaking changes with
recovery flags.
