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

Expected on the 0.1.0 scaffold baseline: **0 pass / 0 failed / 0 ignored**
(no tests yet — the scaffold ships an empty test surface).

As XTR domain code lands, this count grows and the baseline in
`HANDOFF.md` gets updated with each release.

Next: [What to read next](./next-steps.md).
