# Decision record: XSD validation of raw-XML lanes

Status: accepted (2026-11-12).

## Context

A raw-XML lane receives bare XML from a peer and sends bare XML to a peer.
The ERRU XML Message Reference (section 6) says that the receiver must check
that a message is well-formed and must validate it against its XSD.

XTR already reads the XSD files of a lane. The model that it builds
(`src/inbound/contract.rs`) is a codec hint. It is not a validating schema
model: it does not keep `xs:choice`, attributes or facets.

## Options

| Option | Description | Result |
|--------|-------------|--------|
| a | libxml2 in the XTR process | Refused |
| b | libxml2 in a separate, sandboxed worker process | Refused |
| c | A Java (Xerces) sidecar | Refused |
| d | A general pure-Rust XSD crate | Not now |
| e | No validation at run time; contract tests in CI only | Refused |
| f | The backend validates the message | Refused |
| g | A validator in XTR, in Rust, for a declared subset of XSD 1.0 | Accepted |

Reasons:

* **a.** libxml2 is a C library that would parse every unauthenticated
  message in the process that holds the X-Road keys. Its schema parser loads
  `xs:include` and `xs:import` targets itself. Unless the caller restricts it,
  it substitutes entities and can open network locations. A second parser
  also reads the message differently from the XTR parser (for example, it
  obeys the `encoding` declaration). Two readings of one message are a
  security risk.
* **b.** The isolation needs a separate build of libxml2, a sandbox and own
  vulnerability tracking. The cost is too high for the size of the subset
  that the lanes need.
* **c.** A Java runtime only for validation adds a second stack to run and
  patch.
* **f.** The backend is outside XTR. XTR must not forward a message that it
  has not validated.
* **d.** The tested crate gave wrong results for unprefixed type references
  in a schema with a default namespace. Reconsider when a crate passes the
  differential test below.
* **e.** The specification requires validation on receipt.

## Decision

1. A new module `src/xsdval/` builds its own validation model. It uses the
   same XSD documents that the contract loader already resolved: same bytes,
   same include resolution, same root directory. The codec model does not
   change.
2. XTR parses a message once (`dom::parse_message`). The codec and the
   validator read the same element tree.
3. The validator checks inbound messages and outbound requests. For an
   outbound request, it checks the serialized bytes that go to the peer.
   Validation of peer responses is a separate decision.
4. Configuration, per lane, in the sidecar:

   ```yaml
   validate:
     inbound: enforce    # off | report | enforce
     outbound: enforce
   ```

   A missing key means `enforce`. `report` logs the errors and does not
   refuse the message. `xtr doctor` reports `off` and `report` as
   WEAK. A fourth mode, `annotate`, applies to inbound only (point 5a).
5. Answers: an invalid message gets HTTP 422 `schema_invalid`. An invalid
   outbound request does not go to the peer. An internal validator error gets
   HTTP 500 `validator_failed`; the details go to the log only.
   HTTP 422 is an HTTP-level answer. Use `enforce` on inbound only where the
   protocol of the peer accepts a 4xx answer. A protocol that needs an
   application-level error message for an invalid message (for example the
   ERRU `ErrorNotification` with the status `InvalidFormat`) uses
   `inbound: annotate` (see point 5a). `report` is the step before it.
5a. Mode `annotate` (inbound only): XTR always calls the backend and adds
   `schemaValid` and `violations` to the JSON. The backend owns the answer
   to the peer and the protocol, for example the ERRU `ErrorNotification`.
   XTR stays neutral to the protocol. What goes where:

   | Input | XTR does |
   |-------|----------|
   | Valid message | `POST <backend>/<Message>`, `schemaValid: true` |
   | Message that breaks the XSD | `POST <backend>/<Message>`, `schemaValid: false`, violations, `rawBase64`, no `request`, no `rawXml` |
   | Not UTF-8, bad prolog or declaration, DOCTYPE, unknown root | `POST <backend>/_invalid`, `schemaValid: false`, one code, `rawBase64` |
   | Body over the limit, `XTR_OFFLINE` | HTTP 413, HTTP 503 |
   | Validator failure | HTTP 500 `validator_failed` |

   Rules: `annotate` cannot be used for `outbound`. A message with `forward`
   cannot use `annotate`. `inbound.backend` is required, and no message is
   named `_invalid`. Invalid input is only sent as `rawBase64`, because it
   comes from an unauthenticated sender and can be invalid UTF-8; the
   backend must not parse it with an entity-resolving parser. `xtr doctor`
   reports the lane as INFO, not WEAK, because the backend owns the decision.
   With `annotate`, every request body that reaches the lane goes to the
   backend. In production, the inbound port is restricted to the peer
   (ingress mTLS or a network rule).
6. An error names the element path, a rule code and the schema file and line.
   It never contains the value from the message. The file name is relative
   to the schema directory. An answer has at most 10 errors.
7. Supported subset: `xs:include`, `xs:sequence`, `xs:choice`, `xs:extension`,
   `xs:restriction`, `xs:attribute`, `xs:attributeGroup`, the facets
   `enumeration`, `whiteSpace`, `length`, `minLength`, `maxLength`,
   `minInclusive` and `pattern`, `fixed`, and the built-in types `string`,
   `token`, `boolean`, `nonNegativeInteger`, `positiveInteger`, `date` and
   `dateTime`.
8. Fail closed at boot: when validation is not `off`, a construct outside the
   subset that a lane message can reach (also through type derivation,
   attribute groups or includes) stops the boot. The boot error names the
   file, the line and the construct.
9. Content models: occurrence counters, no expansion of `maxOccurs`. At each
   decision point, the possible next elements (the follow set, by qualified
   name, across sequence and extension boundaries and through groups that can
   be empty) must be distinct. Otherwise the model is not supported.
10. Patterns: a parser for the XSD regular-expression grammar (XSD 1.0,
    Appendix F) builds a syntax tree and emits a `regex` expression with an
    explicit anchor. `.`, `\s`, `\w` and `\d` use the XSD definitions, not
    the `regex` definitions. Constructs that XSD does not define, character-class
    subtraction and `\p{Is...}` blocks are errors. Patterns compile at boot
    only, with size limits. Patterns of one derivation step combine with OR;
    patterns of different steps combine with AND.
11. `whiteSpace` applies before the other facets. Text in element-only
    content is invalid. `xsi:type` and `xsi:nil` in a message are invalid.
    `xsi:schemaLocation` and `xsi:noNamespaceSchemaLocation` are ignored.

## Gate

Before `enforce` is the default in production, a differential test compares
the validator with Xerces on the ERRU sample messages and on generated invalid
messages. The test must show zero differences. Xerces runs only in the
test, not at run time. Until then, use `report` in
test environments.

## Consequences

* New dependency: `regex` (pure Rust, linear-time matching).
* No C code is added to XTR.
* A schema set that uses constructs outside the subset cannot use validation
  until the subset grows. The boot error shows what to add.
