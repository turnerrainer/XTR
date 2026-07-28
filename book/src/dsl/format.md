# DSL format

XTR-on-Rust reads YAML "DSL" files that map REST endpoints to
X-Road SOAP calls. One file per service.

## Where files live

`config.dsl_path/<group>/<service>.yml` — the directory name
becomes the URL's `group` segment, the filename stem becomes
`service`. `POST /<group>/<service>` invokes it.

The shipped `DSL/` folder demonstrates the convention:

```
DSL/
├── ar/
│   ├── lihtandmed_v3.yml                → POST /ar/lihtandmed_v3
│   ├── detailandmed_v2.yml              → POST /ar/detailandmed_v2
│   ├── ettevottegaSeotudIsikud_v1.yml   → POST /ar/ettevottegaSeotudIsikud_v1
│   └── tegelikudKasusaajad_v2.yml       → POST /ar/tegelikudKasusaajad_v2
└── xroad/
    ├── listMethods.yml                  → POST /xroad/listMethods
    └── allowedMethods.yml               → POST /xroad/allowedMethods
```

## File shape

```yaml
params:
  - reg_code
service: https://ariregxmlv6.rik.ee/     # optional — see below
method: POST
envelope: >
  <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/">
    <soapenv:Body>
      <prod:ettevottegaSeotudIsikud_v1>
        <prod:keha>
          <prod:ariregistri_kood>{{reg_code}}</prod:ariregistri_kood>
        </prod:keha>
      </prod:ettevottegaSeotudIsikud_v1>
    </soapenv:Body>
  </soapenv:Envelope>
```

### `params`

Allow-list of parameter names permitted in the request body.
Any JSON key **not** on this list is silently dropped **before**
Handlebars sees the request — this is what prevents template
injection from a hostile REST caller.

### `service`

- **Set** (a URL): XTR sends the expanded envelope directly to
  that URL via plain HTTPS (system trust store). Used for
  services that don't require X-Road membership — e.g. the
  Ariregister public XML feeds.
- **Absent**: XTR routes the envelope through the configured
  X-Road Security Server using mTLS with the PKCS12 keystore.
  If `security_server` isn't configured in `xtr.yaml`, this
  DSL will error at request time with a clear message.

### `method`

`GET` or `POST`. The outbound HTTP method to use when calling
the upstream. Typically `POST` for real X-Road.

### `envelope`

The SOAP envelope template. Handlebars `{{name}}` placeholders
are substituted from two sources:

- **User params** (from the request body, filtered against
  `params:`).
- **Auto-context**, always available:
  - `{{generate.uuid}}` — random UUID per request (X-Road
    message id).
  - `{{generate.instance}}` — X-Road instance name from config
    (`xroad_instance:`).
  - `{{generate.client}}` — pre-built `<xroad:client>` element
    with member class / member code / subsystem code.

## Response shape

XTR-on-Rust returns:

```json
{
  "body":    { …translated SOAP <Body> contents… },
  "headers": { …translated SOAP <Header> contents… }
}
```

Both are surfaced. (The original JVM XTR dropped `<Header>` on
the floor; this is a deliberate fix — see
[DESIGN.md §7 bug #7](https://github.com/turnerrainer/XTR/blob/dev/docs/DESIGN.md#7-known-bugs-and-rough-edges-in-the-jvm-version).)

## Example — end to end

`DSL/ar/ettevottegaSeotudIsikud_v1.yml`:

```yaml
params:
  - reg_code
service: https://ariregxmlv6.rik.ee/
method: POST
envelope: >
  <soapenv:Envelope xmlns:soapenv="http://schemas.xmlsoap.org/soap/envelope/"
                    xmlns:prod="http://arireg.x-road.eu/producer/">
    <soapenv:Body>
      <prod:ettevottegaSeotudIsikud_v1>
        <prod:keha>
          <prod:ariregistri_kood>{{reg_code}}</prod:ariregistri_kood>
        </prod:keha>
      </prod:ettevottegaSeotudIsikud_v1>
    </soapenv:Body>
  </soapenv:Envelope>
```

Request:

```bash
curl -sX POST http://localhost:8080/ar/ettevottegaSeotudIsikud_v1 \
     -H 'Content-Type: application/json' \
     -d '{"reg_code": 70006317}'
```

Response (real Ariregister returns a large XML tree; abbreviated):

```json
{
  "body":    { "prod:ettevottegaSeotudIsikud_v1Response": { … } },
  "headers": {}
}
```

## For contributors adding a new DSL

1. Drop the YAML file under `<dsl_path>/<group>/<service>.yml`
2. Restart XTR (no hot-reload in v0.2.0-rc.1 — tracked as a
   `v0.3` roadmap item in DESIGN.md)
3. `GET /api` now lists the new operation
4. Test via `curl`

Names must be URL-safe. Estonian characters in file/dir names
technically work but are best avoided for cross-platform sanity.
