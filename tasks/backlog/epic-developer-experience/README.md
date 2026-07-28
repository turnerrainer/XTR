# Epic — Developer experience

**What**: quality-of-life features for the humans authoring DSLs
against XTR-on-Rust. Not correctness, not security — the ergonomics
layer that makes the difference between "1 hour per new service" and
"5 minutes per new service."

**Why**: XTR's value comes from how many services get wrapped in it.
Every friction point in the DSL authoring loop (hand-writing SOAP
envelopes, guessing param names from stack-trace fault codes,
restarting for every edit) scales up as more services get added.
Fixing DX pays back with every DSL after the first.

**Closes when**:

- Common DSL authoring tasks (write from a WSDL, iterate on an
  envelope, reload without restart) each take under a minute.
- A first-time DSL author can go from "here's my WSDL" to a working
  `POST /group/service` without hand-typing SOAP XML.

**Open tasks**: see sibling `NNN-*.md` files.

**Related**: `book/src/dsl/adding-a-service.md` (the current
hand-written walkthrough — reduces its length when this epic's
tasks land).
