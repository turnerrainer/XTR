# Adding a new service

You want XTR to expose one more X-Road (or plain SOAP) service as a
REST endpoint. This chapter walks the full loop: figuring out the
SOAP shape → writing the DSL → verifying it loads → hitting it with
curl → debugging when it doesn't work.

If you've never touched XTR before, run through
[Run it locally](../getting-started/run-locally.md) first so you have
a working baseline. If you're adding a real X-Road service (not a
public XML endpoint), you also need a running Security Server — see
[X-Road Security Server setup](../ops/xroad-security-server.md).

## Step 0 — decide which path

| Your target | XTR path | What you need |
|---|---|---|
| Public HTTPS SOAP endpoint (Ariregister etc) | Plain HTTPS | Vendor's SOAP shape (WSDL, docs, or a working curl example) |
| Real X-Road service in `ee-test` or `EE` | mTLS via your SS | The target's subsystem identity + `listMethods` output |

Both paths use the same DSL format; the only difference is whether
you set `service:` (URL of the direct endpoint) or leave it absent
(so XTR routes through your Security Server).

## Step 1 — get the SOAP envelope shape

You need to know:
- The target's XML namespace(s)
- The operation element name (`<prod:someOperation>` etc)
- What fields go inside its body

### For a public SOAP service

Three usable sources, in decreasing order of reliability:

1. **The vendor's WSDL** — usually at `<service-url>?wsdl`.
   Every `wsdl:operation` becomes a candidate XTR service; the
   `wsdl:message` / `xsd:complexType` tells you the field names
   and types. Save the WSDL locally, open in any editor.
2. **The vendor's docs / a shipped SDK** — often more readable
   than the WSDL and calls out required vs optional fields.
3. **A working request captured with `curl -v` or a proxy** —
   fall back only if the above don't exist. Pretty-print the
   captured XML and copy the envelope structure.

For Ariregister specifically, the WSDL is public and the operation
names (`lihtandmed_v3`, `detailandmed_v2`, etc) map straight to the
`<prod:*>` element names in the body.

### For a real X-Road service

Two-step:

1. Point XTR (or `curl` directly through your SS) at the target's
   `listMethods`. The response enumerates the target's service
   codes.
2. For a specific service code, ask the target for its WSDL — real
   X-Road services expose WSDLs via `getWsdl` or a documented
   catalog URL (varies by target). You get the same information
   as the public-WSDL case.

The two shipped `xroad/listMethods.yml` and `xroad/allowedMethods.yml`
DSLs already do step 1 for you — invoke them with your target's
member class / code / subsystem code as JSON body.

## Step 2 — write the DSL

Create `<dsl_path>/<group>/<service>.yml`. The `<group>` becomes the
first URL segment; `<service>` (the filename stem) becomes the
second. So a file at `DSL/samples/ar/my_lookup.yml` responds at
`POST /ar/my_lookup`.

Minimum viable DSL:

```yaml
params:
  - reg_code
service: https://example.com/soap
method: POST
envelope: >
  <?xml version="1.0" encoding="utf-8"?>
  <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"
                    xmlns:prod="http://example.com/producer/">
    <soapenv:Body>
      <prod:myLookup>
        <prod:reg_code>{{reg_code}}</prod:reg_code>
      </prod:myLookup>
    </soapenv:Body>
  </soapenv:Envelope>
```

Field-by-field:

| Field | Purpose |
|---|---|
| `params:` | Allow-list of JSON keys the caller may supply. Anything else is silently dropped before Handlebars sees it — this is your template-injection defense. Only list what your envelope actually references. |
| `service:` | Set to a URL for plain HTTPS. **Omit** to route through the configured Security Server (mTLS). |
| `method:` | Almost always `POST` — SOAP-over-HTTP convention. |
| `envelope:` | The full SOAP envelope as a Handlebars template. See below for auto-context placeholders you can use in addition to your `params`. |

Full reference in [DSL format](./format.md).

### Handlebars pitfalls (the two that bite)

**Escaping**: `{{expr}}` HTML-escapes the value (`<` → `&lt;`).
`{{{expr}}}` (triple-brace) does not. For user-supplied *text*
values (numeric codes, names), double-brace is right and safe. For
values that are themselves *XML fragments* (like `{{generate.client}}`
which is `<xroad:client>…</xroad:client>`), you MUST use triple-brace
or the SS will reject an envelope full of `&lt;xroad:client&gt;`.

**Whitespace inside braces**: `{{ foo }}` works;
`{{generate.  client}}` does not — Handlebars rejects paths with
embedded whitespace. Startup validation catches this at boot, but
copying and pasting can still bite you.

### X-Road envelope pattern

For real X-Road services, your envelope needs the standard header
block. Reuse the auto-context:

```yaml
envelope: >
  <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"
                    xmlns:xroad="http://x-road.eu/xsd/xroad.xsd"
                    xmlns:id="http://x-road.eu/xsd/identifiers">
    <soapenv:Header>
      {{{generate.client}}}
      <xroad:service id:objectType="SERVICE">
        <id:xRoadInstance>{{generate.instance}}</id:xRoadInstance>
        <id:memberClass>{{member_class}}</id:memberClass>
        <id:memberCode>{{member_code}}</id:memberCode>
        <id:subsystemCode>{{subsystem_code}}</id:subsystemCode>
        <id:serviceCode>myOperation</id:serviceCode>
      </xroad:service>
      <xroad:id>{{generate.uuid}}</xroad:id>
      <xroad:protocolVersion>{{generate.protocol_version}}</xroad:protocolVersion>
    </soapenv:Header>
    <soapenv:Body>
      <prod:myOperation .../>
    </soapenv:Body>
  </soapenv:Envelope>
```

Auto-context available in every envelope:

| Placeholder | What it renders |
|---|---|
| `{{generate.uuid}}` | Fresh UUID per request — X-Road message id |
| `{{generate.instance}}` | `xroad_instance:` from config (e.g. `ee-test`) |
| `{{{generate.client}}}` | Your `<xroad:client>` element built from `client_data:` — **triple-brace, always** |
| `{{generate.protocol_version}}` | `xroad_protocol_version:` from config (default `"4.0"`) |

## Step 3 — verify it loads

Restart XTR. On boot the loader validates every DSL's envelope
against Handlebars; if yours has a syntax error, XTR fails to start
with the offending file path in the error message. Fix and restart.

Once startup succeeds:

```bash
curl -s http://localhost:8080/api | jq '.paths | keys' | grep my_lookup
```

Should list your new path. If it doesn't, check that:
- The file has a `.yml` or `.yaml` extension.
- The file is inside `<dsl_path>` (whatever `dsl_path:` says in
  `xtr.yaml`, or the built-in default `./DSL`).
- The directory name isn't reserved / has no typos.

## Step 4 — hit it

```bash
curl -sX POST http://localhost:8080/ar/my_lookup \
     -H 'Content-Type: application/json' \
     -d '{"reg_code": "70006317"}'
```

Success looks like:

```json
{
  "body":    { "prod:myLookupResponse": { ... } },
  "headers": { ... any SOAP <Header> content ... }
}
```

For X-Road envelope-based services, `headers` will contain the
requestId, client identity echo, and protocolVersion — proof that
the full X-Road round-trip worked.

## Step 5 — when it doesn't work

XTR's error responses are structured — look at the `error` field
first. Full table in [Failure modes](../ops/failure-modes.md).

Most common failures when developing a new DSL:

| Symptom | Likely cause | Fix |
|---|---|---|
| 502 `upstream_soap_fault` code = `Client.MissingParam` (or similar) | Your envelope is missing a required field, or a param the caller supplied got dropped because it wasn't in your `params:` allow-list | Add the field to `params:` and to the envelope; recheck the WSDL for required fields |
| 502 `upstream_soap_fault` code = `Server.InvalidRequest` or similar | Envelope structure is wrong (bad namespace, wrong element order, wrong nesting) | Compare byte-for-byte against a known-working request. Enable `RUST_LOG=xtr_on_rust=debug` to see the outbound envelope |
| 502 `upstream_http_error 401/403` | X-Road path: your subsystem isn't authorized to call this service | Ask the target's service owner to add your subsystem to their allow-list |
| 200 but response body is `<html>…</html>` | You pointed `service:` at a web page URL, not a SOAP endpoint | Double-check the WSDL's `<soap:address location="…"/>` |
| 502 `upstream_xml_parse_error` | Upstream returned something that isn't XML | Usually a proxy / auth gateway intercepted the request. Same debugging: log the outbound |
| Values render as `&lt;something&gt;` in the outbound envelope | You used `{{value}}` where you needed `{{{value}}}` | Triple-brace the placeholder that holds raw XML |

### Debugging with logs

Turn up log verbosity temporarily:

```bash
RUST_LOG=xtr_on_rust=debug ./xtr-on-rust --config xtr.yaml
```

The `plain HTTPS POST <url>` / `SS mTLS POST <url>` log lines tell
you exactly what URL XTR is calling. To see the actual envelope,
add a temporary `println!` in `src/router/mod.rs` — the current
release doesn't dump envelopes at any log level (they can contain
credentials like the Ariregister password param).

## Where to go next

- [DSL format](./format.md) — full reference for the four fields.
- [X-Road Security Server setup](../ops/xroad-security-server.md) —
  needed when your DSL omits `service:`.
- [Failure modes](../ops/failure-modes.md) — every status code XTR
  can return and what causes it.

## What XTR does NOT do (that a naive reader might expect)

- **Auto-generate DSLs from WSDL.** Every DSL is hand-written. A
  `xtr scaffold` CLI that ingests a WSDL and emits a candidate DSL
  is a filed feature request — see
  [task 013](https://github.com/turnerrainer/XTR/blob/dev/tasks/backlog/epic-developer-experience/013-wsdl-to-dsl-scaffold-generator.md).
- **Hot-reload DSL files.** Restart the process to pick up changes.
- **Validate response schemas.** XTR translates whatever the
  upstream returned; it doesn't check that it matches the WSDL's
  response type.
- **Discover services automatically.** You point XTR at services
  you know exist. `listMethods` is a manual step you take when
  building a new DSL.
