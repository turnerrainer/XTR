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
One leading BOM is allowed. A declaration without `encoding`, or no
declaration, is accepted.

## Layout and sidecar

```text
wsdl/<group>/<name>.xml.yaml     # the sidecar (<group> subdirectory is required)
wsdl/<group>/*.xsd               # entry XSDs + local includes/imports
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

## Not covered

* **XSD validation.** The schema is a codec hint (order, repetition,
  namespaces). Required elements, enumerations and facets are **not**
  checked; validate in the backend/caller if the peer requires it.
* Sync replies in the response body, SOAP, WS-Security, retries, queues.
* `xtr doctor` only counts `.xml.yaml` lanes for the shared-listener
  exposure check; sidecar contents are validated at boot.
