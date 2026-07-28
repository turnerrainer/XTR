# Epic — Testing infrastructure

**What**: the fixtures, mock servers, and CI plumbing that make
XTR-on-Rust testable end-to-end without a real X-Road membership.

**Why**: XTR's job is to translate REST → SOAP → X-Road →
XML → JSON. That's not unit-testable in a meaningful way — every
seam matters. Real X-Road access is gated on membership +
production certs. Without a solid mock layer, we're either
testing nothing meaningful or blocking CI on external state.

**Closes when**:

- Every shipped DSL sample has a corresponding integration test.
- CI can prove end-to-end request→response mapping without any
  network dependency.
- Failures in the mock layer produce diagnostic output that maps
  1:1 to what a real X-Road failure would show.

**Open tasks**: see sibling `NNN-*.md` files.

**Related**: DESIGN.md §2.7, `tests/` directory.
