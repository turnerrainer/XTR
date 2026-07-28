# Watch the automated tests pass

Needs the [Rust toolchain](./prerequisites.md).

## Build

```bash
cargo build --release --bin xtr-on-rust
```

First build is a few minutes; incremental builds are seconds.

## Rust unit + integration tests

```bash
cargo test --no-fail-fast
```

Expected on the pre-`v0.2.0-rc.1` baseline (task 002 in
progress): **29 passed / 0 failed / 0 ignored** (24 unit tests
across config, dsl loader, handlebars, translate, openapi + 5
integration tests exercising the full HTTP surface with an
in-process mock upstream).

As XTR domain code lands, this count grows and the baseline in
`HANDOFF.md` gets updated with each release.

Next: [What to read next](./next-steps.md).
