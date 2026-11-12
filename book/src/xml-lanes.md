# Raw-XML lanes (no SOAP envelope)

Some peers exchange bare XML documents over plain HTTP(S) — no SOAP
envelope, no WSDL. A typical example is the EU ERRU MOVEHUB
`BTSHTTPReceive` interface. The [SOAP lanes](./soap-lanes.md) cannot
talk to such a peer: they always wrap the payload in a SOAP 1.1 envelope
and need a WSDL.

The raw-XML lanes reuse the SOAP lanes' XSD-guided XML ⇄ JSON codec but
put the **document itself** on the wire. They are opt-in per contract
through a sidecar `<name>.xml.yaml` next to the XSD files and do nothing
without one.

| Direction | Who calls whom | Endpoint |
|---|---|---|
| **inbound** | the peer POSTs raw XML to XTR; XTR POSTs JSON to your backend | `POST /xml-in/<group>/<name>` (and an optional alias, see `inbound.path`) |
| **outbound** | your service POSTs JSON to XTR; XTR POSTs raw XML to the peer | `POST /xml-out/<group>/<name>/<message>` |

As with the SOAP lanes the prefixes differ on purpose: `/xml-in/` is for
the peer network, `/xml-out/` makes XTR act with its own identity and is
covered by `XTR_INTER_SERVICE_TOKEN` — never expose it to the peer
network. The same exposure rule applies: when inbound and outbound share
one listener, set `inbound.port` or `XTR_INTER_SERVICE_TOKEN`.

> **`/xml-in/` has no authentication.** This includes every `inbound.path`
> alias. XTR does not check a token, a certificate or a source address on
> these routes. Restrict them at the ingress (IP allowlist or mTLS). Set
> the rate limit at the ingress. Each accepted request makes one call to
> the backend.

> **XTR stays stateless.** There is no queue, retry, outbox or deadline
> handling. XTR hands the backend the received bytes and their SHA-256
> digest. The digest proves the integrity of the received bytes and
> supports audit. It does not detect duplicates: the backend detects them
> with the message ids (`technicalId`, `workflowId`). Replay detection,
> persistence and retries are the backend's / caller's job.

**Encoding and prolog.** The MOVEHUB ERRU XML Message Reference (v2.06,
section 5.7.2) allows UTF-8 only. The Hub discards any other encoding
declaration. XTR refuses a message with HTTP 400 in these cases:

* The XML declaration names an encoding that is not `UTF-8` or `utf8`
  (any case): `unsupported_encoding`. The check also runs before `forward`.
* The prolog is not well-formed XML: the XML declaration is not the first
  item (a space, a comment or a second BOM comes before it), it is repeated,
  its `version` is not 1.0 or 1.1, or it comes after the root element.
  A processing instruction named `xml` (any case) is refused. Text outside
  the root element is refused, except white space: `malformed_xml`.

The declaration follows XML 1.0 §2.8: `version`, then an optional
`encoding`, then an optional `standalone`, each once, in lower case.
`utf8` is accepted as an alias of UTF-8. A `forward` target with a strict
XML parser can refuse it.

One leading BOM is allowed. A declaration without `encoding`, or no
declaration, is accepted.

## Layout and sidecar

```text
wsdl/<group>/<name>.xml.yaml     # the sidecar (<group> subdirectory is required; `.xml.yml` also works)
wsdl/<group>/*.xsd               # entry XSDs + local includes (see "Supported XSD" for imports)
```

`<group>/<name>` become the endpoint names. Nested directories are joined
with `-`, like the SOAP lanes.

```yaml
schemas: [NotifyUnfitness_Request.xsd, NotifyUnfitness_Acknowledgement.xsd]

# Message types this contract carries = global element names in the XSDs.
# Anything else is refused. `Name: {}` is enough.
messages:
  NotifyUnfitness_Request:
    direction: both          # in | out | both (default); a one-way message should say so
  NotifyUnfitness_Acknowledgement:
    forward: http://legacy-receiver.internal:8080/xml       # inbound only, see "Staged migration"

inbound:
  backend: http://backend.internal:8080/erru/xml-in   # POST <backend>/<Message>
  success_status: 202        # status returned to the peer on a 2xx backend answer (default 202)
  include_raw: true          # also send `rawXml` to the backend (default true)
  relay_statuses: [400, 409, 422]   # backend 4xx codes handed to the peer as they are (default)
  path: /erru/xml            # optional extra route serving this lane

outbound:
  url: https://peer.example/http/request/BTSHTTPReceive.dll   # per message: messages.X.url
  timeout_secs: 15           # 1..=limits.request_timeout_secs; per message: messages.X.timeout_secs
  keystore_path: /app/ssl/own.p12          # optional client certificate (PKCS#12)
  keystore_password_env: XTR_OUTBOUND_KEYSTORE_PASSWORD
  trust_ca_path: /app/ssl/peer-ca.pem      # optional
  content_type: application/xml; charset=utf-8

validate:                    # XSD validation, see "XSD validation"
  inbound: report            # off | report | enforce (default enforce)
  outbound: report           # do not copy enforce to production before "Turning on enforce or annotate"
```

Boot is refused, and XTR prints the full list of faults, for each of
these cases:

* An XSD file is missing or cannot be parsed.
* A `messages` entry is not a global element.
* A lane has no `inbound` and no `outbound` section.
* A message has no backend URL and no peer URL.
* An `inbound.path` is not a literal absolute path, uses a reserved
  prefix, or would shadow a DSL endpoint `/:group/:service`.
* `success_status` is not a 2xx status.
* A `relay_statuses` entry is not a 4xx status.
* A `direction: out` message has `backend` or `forward`, or a
  `direction: in` message has `url` or `timeout_secs`.
* A message `direction` needs an `inbound:` or `outbound:` section that
  the sidecar does not have, or a section exists that no message can use.
* A message name is a global element in more than one namespace.
* A `schemas:` entry contains a path separator.
* `outbound.content_type` is not valid.
* `timeout_secs` is out of range.
* The `validate:` section has an unknown key or an unknown mode.
* `inbound.backend_headers_env` has a variable without the prefix
  `XTR_BACKEND_HEADER_`, a forbidden or repeated header name, or a
  variable that is not set or is empty (see "Backend headers").
* An XSD construct is outside the supported XSD subset while validation
  is on. This includes a construct in an imported or included file.
* A peer URL fails the SSRF guard that also checks WSDL-declared URLs
  (`wsdl.allow_http_upstream`, `wsdl.upstream_host_allowlist`).

## Inbound

1. XTR reads the body. The body must be within
   `limits.max_request_bytes`. Otherwise XTR answers 413.
2. XTR checks the body. The body must be UTF-8 XML. XTR answers 400
   `unsupported_encoding` for an XML declaration with another encoding.
   XTR answers 400 `malformed_xml` for a prolog that is not well-formed.
   See "Encoding and prolog" above. A `DOCTYPE` or DTD is also rejected
   with 400. This rule prevents XXE and entity expansion.
3. The root element (local name **and** namespace) must be one of
   the `messages` that can arrive (`direction: in` or `both`).
   Otherwise XTR answers 400 `unsupported_message`.
4. XTR decodes the document to JSON with the XSD as a hint (see
   [codec conventions](./soap-lanes.md): `@attr` keys, arrays for
   repeated elements, text never coerced).
5. XTR POSTs to the backend:

   ```json
   {
     "service": "erru/erru",
     "message": "NotifyUnfitness_Request",
     "request": { "Header": { "@technicalId": "…" }, "Body": { … } },
     "sha256": "9f2c…",          // SHA-256 (hex) of the exact bytes received
     "rawXml": "<?xml …"         // omitted when include_raw: false
   }
   ```

   plus headers `x-xtr-inbound-service`, `x-xtr-message` and the
   incoming `traceparent`.
6. The backend decides the answer the peer sees:

   | Backend | Peer gets |
   |---|---|
   | 2xx | `success_status` (default 202), empty body |
   | 4xx listed in `relay_statuses` (default 400, 409, 422) | the same status and the backend's body, always sent as `Content-Type: application/json` |
   | any other 4xx (wrong backend path 404, gateway guard 401/403 …) | 502 `backend_error` — an infrastructure fault is not a verdict on the message, so the peer retries |
   | 5xx / unreachable / timeout | 502 / 502 / 504, no backend details |
   | `XTR_OFFLINE` | 503 `offline` |

   The backend owns the body of a relayed 4xx. XTR sends it to the peer
   as it is, and always with `Content-Type: application/json`, also when
   the backend sent another type. The peer can be anyone who reaches
   `/xml-in/`. The backend must not put internal details (stack traces,
   SQL errors, internal URLs) into the body of a status listed in
   `relay_statuses`. XTR cuts a relayed body that is over the response
   limit (`limits.max_response_bytes`). The cut JSON can be incomplete.

   A peer that retries a failed delivery meets the backend's own
   idempotency (the message ids in `request`). Use `sha256` to check
   that two deliveries carry the same bytes.

### Staged migration: `forward`

`messages.X.forward: <url>` makes XTR POST the **received bytes
unchanged** (same `Content-Type`) to `<url>`. XTR relays the status and
the body of the answer to the peer. XTR does not decode or store the
message.

Use `forward` to move one message type at a time from an old receiver to
a new backend. List every message type. Give each message type that the
old system still serves a `forward`. Give the other message types a
backend. `forward` and `backend` are mutually exclusive per message.

When inbound validation is `enforce`, XTR checks the message before it
forwards it. An invalid message no longer reaches the old system. See
"XSD validation".

**Which answers XTR relays.**

* A 2xx answer is relayed.
* A 4xx code listed in `inbound.relay_statuses` (default 400, 409, 422)
  is relayed.
* Any other status becomes a plain 502 `forward_error`. This includes
  3xx, 5xx, and codes such as 401 and 404. These codes usually mean a
  wrong target URL or wrong credentials, not a verdict on the message.
* An unreachable target also becomes 502 `forward_error`. The details
  only reach the log.
* An answer with an unreadable body, or a body over the response limit,
  is relayed with an empty body. XTR keeps the status and logs a warning.
* XTR keeps the `Content-Type` of the target. The answer can be an empty
  `application/xml`. (A relayed 4xx from a *backend* is different: XTR
  cuts that body, see the table above.)

**Limits of `forward`.**

* While `XTR_OFFLINE` is set, XTR blocks the forward (503 `offline`).
* XTR checks only the scheme of the target URL, like backend URLs.
* The SSRF guard of the outbound lane does not apply to `forward` and
  `backend` URLs. They are operator configuration.
* The forward client and the backend client have no client certificate
  and no custom CA. Put an internal hop in front of a target that needs
  mTLS or a private CA.

**Loop guard.** XTR adds the header `x-xtr-forwarded: 1` to every forwarded
request. A message with a `forward` that arrives with this header is refused
with 508 `loop_detected` before any outbound call. This stops a `forward`
that points back to an XTR `/xml-in/` route. A chain where the second XTR
forwards the message again is refused with 508. A chain where the second
XTR sends the message to its backend works.

## Outbound

`POST /xml-out/<group>/<name>/<message>` with a JSON object — the content
of the message's root element, same conventions as the inbound `request`.
XTR encodes it (child order follows the XSD; `null` omits), prepends the
XML declaration and POSTs it to the peer with `Content-Type:
application/xml; charset=utf-8` and the per-message timeout.

The **upstream answer is data, not an error**: XTR returns HTTP 200 with

```json
{
  "status": 202,                 // the peer's HTTP status, whatever it is
  "contentType": "application/xml",
  "body": "<…raw reply…>",       // raw text; invalid UTF-8 is replaced, cut at limits.max_response_bytes
  "bodyTruncated": false,        // true when the reply was longer than the limit
  "bodyReadFailed": false,       // true when the connection broke while reading the body
  "response": { "message": "NotifyUnfitness_Acknowledgement", "content": { … } }
}
```

`response` is `null` unless the reply is complete and its root is a
global element of the lane's schemas. A
caller that must tell "the peer said no" (a status) from "we do not know
what happened" (timeout, connection loss) can: transport failures use
an error body — `upstream_timeout` 504, `upstream_unreachable` 502 (no
URL in the response, it is logged). Once the peer has answered, its
status is always returned, whatever the body looks like.

## XSD validation

XTR validates inbound messages and outbound requests against the XSD
of the lane. The validator is written in Rust. It reads the same XSD
files and the same element tree as the codec. XTR does not validate the
responses of a peer.

### Modes

Set the mode for each direction in the sidecar:

```yaml
validate:
  inbound: report    # off | report | annotate | enforce (default enforce); annotate: inbound only
  outbound: report   # a missing key means enforce; see "Rolling out a version with validation"
```

A missing key means `enforce`.

| Mode | Behavior |
|------|----------|
| `enforce` | XTR refuses a message that breaks the XSD. |
| `report` | XTR logs the violations and passes the message on. |
| `annotate` | Inbound only. XTR always calls the backend and tells it the verdict. The backend answers the peer. See "Annotate". |
| `off` | XTR does not validate. |

`enforce` answers HTTP 422. Use it only where the protocol of the peer
accepts a 4xx answer. Some protocols need an application-level error
message for an invalid message. The ERRU protocol is an example: the
receiver answers 200 or 202 and sends an `ErrorNotification` with the
status `InvalidFormat`. Use `inbound: annotate` for such a lane, so that
the backend sends the error message (see "Annotate"). Use `report` as
the step before `annotate`, while the backend does not yet send the
error message. Do not use `enforce` on such a lane.

`xtr doctor` reports a direction with `off` or `report` as WEAK. It
reports a lane with `annotate` as INFO, because the backend owns the
decision. Use
`report` for a limited time, as a step in a roll-out. It is not a
permanent setting for a lane that must refuse invalid messages.

### Annotate

`annotate` is an inbound mode for protocols where the receiver must send
an application-level error message for an invalid message. XTR stays
neutral to the protocol: XTR tells the backend the verdict, and the
backend answers the peer.

**What goes where.**

| Input | XTR does |
|-------|----------|
| Valid message | `POST <backend>/<Message>` with `schemaValid: true`. |
| Message that breaks the XSD | `POST <backend>/<Message>` with `schemaValid: false` and the violations. |
| Not UTF-8, bad prolog or declaration, DOCTYPE, or unknown root | `POST <backend>/_invalid` with `schemaValid: false` and one code. |
| Body over `limits.max_request_bytes` | HTTP 413. XTR does not call the backend. |
| `XTR_OFFLINE` is set | HTTP 503 `offline`. XTR does not call the backend. |
| The validator fails | HTTP 500 `validator_failed`. XTR does not call the backend. |

The peer gets the answer of the backend, as in the other modes (see
`success_status` and `relay_statuses`).

**Body for a valid message.**

```json
{
  "service": "erru/erru",
  "message": "NotifyUnfitness_Request",
  "request": { "Header": { "@technicalId": "…" } },
  "sha256": "9f2c…",
  "rawXml": "<?xml …",
  "schemaValid": true,
  "violations": []
}
```

`rawXml` follows `include_raw`.

**Body for a message that breaks the XSD.**

```json
{
  "service": "erru/erru",
  "message": "NotifyUnfitness_Request",
  "sha256": "9f2c…",
  "rawBase64": "PD94bWwg…",
  "schemaValid": false,
  "violations": [
    {"path": "/NotifyUnfitness_Request/Header/@technicalId", "code": "attribute_missing",
     "line": 1, "schemaFile": "Global_Types.xsd", "schemaLine": 12}
  ]
}
```

* `request` is absent. XTR does not decode an invalid message.
* `rawXml` is absent. `rawBase64` carries the exact bytes. `include_raw`
  has no effect on an invalid message.
* The fields are at the top level of the JSON only. A violation never
  holds a value from the message.

**Body for `<backend>/_invalid`.**

```json
{
  "service": "erru/erru",
  "sha256": "9f2c…",
  "rawBase64": "PGh0bWw+…",
  "schemaValid": false,
  "violations": [{"code": "malformed_xml"}]
}
```

The `message` and `request` fields are absent, because XTR does not
know the message type. The code is one of `malformed_xml`,
`unsupported_encoding`, `doctype` and `unsupported_message`.
`rawBase64` is used because the bytes can be invalid UTF-8.

**Rules for the backend.**

* The backend must not give the content of a message with
  `schemaValid: false` to the business logic.
* The backend must not parse `rawBase64` with an XML parser that
  resolves entities or DTDs. The bytes come from an unauthenticated
  sender.
* The backend must read the identifiers it needs (for example a
  workflow id) with a narrow and bounded method, and must check them.
* The backend must not answer an invalid error message of the protocol
  with a new error message (for example an `ErrorNotification` about an
  `ErrorNotification`). Otherwise the two sides can loop.
* The body of a request is about 4/3 of the size of the message, plus
  the JSON. Check that the body limit of the backend is large enough for
  `limits.max_request_bytes`.

**Boot errors for `annotate`.**

* `validate.outbound` is `annotate`.
* A message has `forward`. `forward` sends the bytes unchanged, so the
  old system would not learn the verdict.
* `inbound.backend` is not set. `_invalid` needs it.
* A message is named `_invalid`.

**Exposure.** With `annotate`, every request body that reaches the lane
goes to the backend, also input that XTR would refuse in the other modes.
In production, restrict the inbound port to the peer (ingress mTLS or a
network rule). `xtr doctor` reports the lane as INFO for this reason.

**Roll-out.** Use `report` first. Move to `annotate` as "Turning on enforce or
annotate" says.

### Backend headers

`inbound.backend_headers_env` makes XTR add headers to the calls to
`inbound.backend`. Use it when the backend must know that a request
comes from XTR, for example behind a gateway guard.

```yaml
inbound:
  backend: http://backend.internal:8080/erru/xml-in
  backend_headers_env:
    x-backend-token: XTR_BACKEND_HEADER_ERRU   # header name: name of the variable
```

The key is the header name. The value is the name of an environment
variable. XTR reads the variable at boot.

* The variable name must start with `XTR_BACKEND_HEADER_`. A sidecar
  must not be able to send another secret (for example a keystore
  password) to its backend.
* These header names are not allowed: `host`, `transfer-encoding`,
  `connection`, `keep-alive`, `te`, `trailer`, `expect`, `upgrade`,
  `traceparent`, `forwarded`, `content-*`, `proxy-*`, `x-xtr-*` and
  `x-forwarded-*`. A name must be a valid HTTP header name.
  The same name twice (not case-sensitive) is an error.
* A variable that is not set, or is empty, stops the boot.
* The header goes only to `inbound.backend`: to the URL of a message
  and to `<backend>/_invalid`. It does not go to a `forward` target, to a
  message with its own `backend` URL, or to a peer.
* XTR marks the value as sensitive. The value is not in a log, in an
  error message or in the output of `xtr doctor`. `xtr doctor` shows
  WEAK when the value is shorter than 32 bytes. It names the variable and
  never prints the value.
* XTR does not follow redirects of the backend, so the header does not
  go to another host.
* Use a random value of at least 32 bytes. Do not commit it.

### Rolling out a version with validation

An existing lane validates from this version on, because a missing key
means `enforce`. Do the steps in this order:

1. Add `validate: report` for both directions to every raw-XML sidecar.
   This applies to `.xml.yaml` and `.xml.yml` files.
2. Run `xtr doctor` of the new version on the sidecar directory. It must
   list every raw-XML lane with `report`.
3. Roll out the new version.
4. Read the logs (see "Logs"). Decide for each violation if the peer or
   the validator is wrong.
5. Turn on `enforce` or `annotate` for one lane and one direction at a
   time. See "Turning on enforce or annotate".

### Turning on enforce or annotate

Do this only when **all** of these conditions are true. They apply to
`enforce` and to `annotate`:

1. A differential test for the schemas of the lane shows 0 differences.
   The test compares XTR with an independent XSD 1.0 processor (for
   example Apache Xerces). The test uses sample messages and generated
   invalid messages. Write down the date and the XTR version.
2. The lane has run in `report` in production for an agreed period (for
   example 14 days). The
   period includes at least one real message of each message type. Every
   logged violation is explained: it is a fault of the peer, or it is a
   fault of the validator and the validator is fixed first.
3. `xtr doctor` lists the lane as `report`. The boot log shows the mode.
4. The log entries of the last period have `suppressed` equal to 0 (see
   "Logs"). A non-zero count means that the log hides violations.
5. For `inbound` with `enforce`: the protocol of the peer accepts an
   HTTP 4xx answer to an invalid message (see "Modes").
6. For `annotate`: the backend handles `schemaValid: false` and
   `<backend>/_invalid` (see "Annotate"). A test with the synthetic
   messages of the contract shows the answer of the backend for each
   case.
7. For `annotate`: the inbound port accepts only the peer (ingress mTLS
   or a network rule). Every request body that reaches the lane goes to
   the backend.

Turn on `outbound` first. An invalid outbound request is a fault of the
own system, and the effect stays with the own caller. Then turn on
`inbound`. Make each direction a separate change.

**What changes with `enforce`.**

* Inbound: XTR answers an invalid message with HTTP 422 `schema_invalid`.
  XTR does not call the backend and does not forward the message. XTR
  does not store the message. A message with a `forward` no longer
  reaches the old system when it is invalid.
* Outbound: XTR answers the caller with HTTP 422 `schema_invalid`. XTR
  sends nothing to the peer.
* The WEAK finding of `xtr doctor` for this direction disappears. The boot does
  not change, because XTR builds the model also for `report`.
* A validator error gives HTTP 500 `validator_failed`. XTR refuses the
  message (fail closed).

**What changes with `annotate`.**

* XTR no longer answers an invalid message itself. The backend answers
  it. The backend must give the peer the answer that the protocol
  requires (for example an `ErrorNotification`).
* XTR also calls the backend for input that the other modes refuse with
  HTTP 400 (see "Annotate").
* The roll back to `report` stops the backend from getting the verdict.
  The backend then gets an invalid message as if it were valid.

**Check after the change.** Read the boot log and the output of
`xtr doctor`. Count the 422 and 500 answers in the first days. If the
count is not as expected, roll back.

**Roll back.** Set the direction to `report` and restart XTR. The roll
back itself loses no data. XTR did not store the messages that it
refused during `enforce`. For an inbound lane, the peer must send again
the messages that XTR refused during `enforce`. This applies only to a
protocol where a 4xx answer makes the peer send again.

**Review.** The change to `enforce` is a separate change with its
own review. Attach the evidence of conditions 1 and 2 to it.

### Inbound

XTR validates the message after the prolog and the root checks and
before it calls the backend and before `forward`.

* With `enforce`, a message that breaks the XSD gets HTTP 422
  `schema_invalid`. XTR does not call the backend and does not forward
  the message.
* With `report`, XTR writes the violations to the log. XTR then handles
  the message as if it were valid: it calls the backend or forwards it.
* With `annotate`, XTR calls the backend for every message that it can
  read, and for unreadable input it calls `<backend>/_invalid`. See
  "Annotate".

The backend can also relay a 422 (see `relay_statuses`). The peer sees
the same status for both cases. The `error` field of the body is the
only difference: it is `schema_invalid` when XTR refuses the message.

### Outbound

XTR validates the serialized request, which are the bytes that go to the
peer.

* With `enforce`, a request that breaks the XSD gets HTTP 422
  `schema_invalid` to the caller. XTR does not send it.
* With `report`, XTR writes the violations to the log and sends the
  request to the peer.

### Answer

The body of a 422 lists at most 10 violations:

```json
{
  "error": "schema_invalid",
  "message": "the message does not conform to its XSD",
  "violations": [
    {"path": "/Notify_Request/Header/@technicalId", "code": "attribute_missing",
     "line": 1, "schemaFile": "Global_Types.xsd", "schemaLine": 12}
  ]
}
```

A violation holds these fields:

* `path`: the element path.
* `code`: a rule code (see "Rule codes").
* `line`: the line in the message. For a missing element
  (`element_missing`), it is the line of the parent element. For an
  attribute violation, it is the line of the element that has the
  attribute. For other codes, it is the line of the start tag of the
  element. The value 0 means that the line is not known.
* `schemaFile` and `schemaLine`: the file and the line in the schema.
  The file name is relative to the schema directory. The value 0 for
  `schemaLine` means that the line is not known.

A violation never holds a value from the message.

An internal error of the validator gives HTTP 500 `validator_failed`.
The details go to the log only. This applies to `enforce` and to `report`.

### Rule codes

| Code | Meaning |
|------|---------|
| `root_unknown` | The root element is not a message of the lane. |
| `element_missing` | A required element is missing. |
| `element_unexpected` | An element is not allowed at this place. |
| `attribute_missing` | A required attribute is missing. |
| `attribute_unexpected` | An attribute is not declared. |
| `attribute_fixed` | An attribute differs from its `fixed` value. |
| `xsi_not_allowed` | The message has `xsi:type` or `xsi:nil`. |
| `text_not_allowed` | An element with element-only content has text. |
| `simple_content_expected` | An element with simple content has a child element. |
| `type_invalid` | The value is not valid for its built-in type. |
| `enumeration` | The value is not in the enumeration. |
| `pattern` | The value does not match the pattern. |
| `length` | The value does not have the required length. |
| `min_length` | The value is shorter than `minLength`. |
| `max_length` | The value is longer than `maxLength`. |
| `min_inclusive` | The value is smaller than `minInclusive`. |

### Supported XSD

XTR supports `xs:include`, `xs:sequence`, `xs:choice`, `xs:extension`,
`xs:restriction`, `xs:attribute`, `xs:attributeGroup`, the facets
`enumeration`, `whiteSpace`, `length`, `minLength`, `maxLength`,
`minInclusive` and `pattern`, `fixed`, and the types `string`, `token`,
`boolean`, `nonNegativeInteger`, `positiveInteger`, `date` and
`dateTime`.

`xs:import` is not supported. XTR treats it as a construct outside the
supported XSD (see "Boot errors").

**`xsi` attributes.** `xsi:type` and `xsi:nil` in a message are violations.
`xsi:schemaLocation` and `xsi:noNamespaceSchemaLocation` are ignored. XTR
never loads a schema from a message.

**Patterns.** A `pattern` follows the XSD grammar: it matches the whole value,
`^` and `$` are plain characters, `.` excludes only line ends, and `\s`,
`\d` and `\w` use the XSD definitions. A construct of the `regex` syntax that
XSD does not define is an error at boot.

### Boot errors

A boot error stops **all of XTR**, not only one lane. The error names the
file, the line and the construct. A boot error occurs in these cases:

* A direction that the lane serves has `report` or `enforce`, and a
  construct outside the supported XSD is reachable from a message root.
* The content model is ambiguous. Two particles with the same qualified
  name stand at one decision point.
* A global definition is duplicate.
* An `xs:include` has another `targetNamespace`, or it is a chameleon
  include.
* An element has `maxOccurs="0"`.
* A type or group definition is recursive.
* A `whiteSpace` facet is looser than the one of the base type.
* An `enumeration` or a length facet has a base type other than `string`
  or `token`.
* A `minInclusive` facet has a base type or a value that does not fit
  the base type.
* A message name is a global element in more than one namespace.
* A `pattern` uses a construct that XSD does not define.
* The `validate:` section has an unknown key or an unknown mode.

To start XTR without validation for a lane, set `validate:` to `off` for
the directions of that lane.

### Logs

Boot logs the validation mode of each lane at info level. A violation
writes a warn entry with the element paths and the rule codes. The entry
never holds a value.

XTR caps the warn log for each lane **and** each direction: at most 10
entries per minute. The first entry of the next minute has a `suppressed`
count of the entries that XTR dropped. The answer to the sender is not
capped.

The same cap applies to the log entries of `annotate`, also for input
that goes to `_invalid`.

## Not covered

* Validation of the responses of a peer.
* XSD constructs outside the supported XSD. Each construct is a separate
  piece of work. See the decision record
  [XSD validation of raw-XML lanes](./adr-xsd-validation.md) for the
  rules to extend the supported XSD.
* Sync replies in the response body, SOAP, WS-Security, retries, queues.
