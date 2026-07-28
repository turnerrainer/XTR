# Epic — Operator onboarding

**What**: everything an ops team needs to actually run XTR-on-Rust
against a real X-Road instance. Certificate acquisition, keystore
lifecycle, environment-specific config, common failure diagnosis.

**Why**: XTR is deceptively simple to build — the hard part for
operators is understanding X-Road itself. This epic hosts the
documentation and tooling that makes onboarding routine.

**Closes when**:

- A new operator can go from "we want to run XTR" to "XTR is
  serving real X-Road traffic" using only what's in this
  project's docs, without needing external X-Road expertise.

**Open tasks**: see sibling `NNN-*.md` files.

**Related**: `book/src/ops/`, DESIGN.md §2.7.
