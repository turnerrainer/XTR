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

Expected on the current baseline: **51 passed / 0 failed / 0
ignored** (41 unit tests across config, dsl loader, handlebars,
translate, openapi + 10 integration tests exercising the full
HTTP surface with an in-process mock upstream). Coverage
includes tasks 003, 005, 010, 011, 012 plus the security-audit
sweep (quick-xml CVE upgrade + XXE guard + nesting-depth cap +
malformed-body handling + path-traversal + Handlebars re-render
regression).

As XTR domain code lands, this count grows and the baseline in
`HANDOFF.md` gets updated with each release.

Next: [What to read next](./next-steps.md).
