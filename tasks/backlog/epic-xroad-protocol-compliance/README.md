# Epic — X-Road protocol compliance

**What**: everything about wire-level correctness of XTR-on-Rust as
an X-Road client. Content-Type discipline, response validation,
protocol version handling, other things beyond mechanical SOAP-over-HTTP.

**Why**: the MVP (task 002 / DESIGN.md §8) proves the mechanical
translation works. This epic proves XTR-on-Rust is actually a
well-behaved X-Road client in front of a real Security Server.

**Closes when**:

- All tasks in this directory are landed.
- XTR-on-Rust can be certified against a real X-Road test instance
  (ee-test) without producing rejects on the wire, and every
  response is validated against the outbound request.

**Open tasks**: see sibling `NNN-*.md` files.

**Related**: DESIGN.md §2.7, §7 (JVM bugs that inspired several
of these tasks).
