# Run it locally

## Path A — pull the pre-built image (once published)

No published image yet. When v0.2.0-rc.1 lands, this becomes:

```bash
docker run -d --name xtr -p 8080:8080 \
    turnerrainer/xtr:0.2.0-rc.1
```

## Path B — build from source

```bash
git clone -b dev https://github.com/turnerrainer/XTR.git xtr
cd xtr
docker compose up -d --build
```

First build is 2–3 minutes; incremental builds are seconds.

## What's running

XTR loads DSLs from `./DSL` at boot. The shipped `DSL/` tree
contains 6 X-Road service mappings (Ariregister + X-Road meta
services). Once task 013 lands, these will be auto-generated
from vendor WSDLs by CI and re-committed on every WSDL change.
Today they're hand-written and match the original JVM XTR
sample set byte-for-byte modulo the fixes documented in
DESIGN.md §7.

## Health check

Request:

```bash
curl http://localhost:8080/health
```

Response:

```json
{"status":"ok"}
```

## Discover what's loaded

Request:

```bash
curl -s http://localhost:8080/api | jq '.paths | keys'
```

Response (with the shipped samples):

```json
[
  "/ar/detailandmed_v2",
  "/ar/ettevottegaSeotudIsikud_v1",
  "/ar/lihtandmed_v3",
  "/ar/tegelikudKasusaajad_v2",
  "/xroad/allowedMethods",
  "/xroad/listMethods"
]
```

## Hit a shipped sample

Ariregister endpoints (`/ar/*`) hit a public XML API — no X-Road
membership required. Try:

```bash
curl -sX POST http://localhost:8080/ar/ettevottegaSeotudIsikud_v1 \
     -H 'Content-Type: application/json' \
     -d '{"reg_code": 70006317}'
```

Response is JSON — the translated SOAP response body + headers.
See the [DSL format chapter](../dsl/format.md) for the response
shape.

X-Road endpoints (`/xroad/*`) route through the configured
Security Server. They need a valid mTLS keystore configured in
`xtr.yaml` — see [Docker](../ops/docker.md) once we ship the
operator-onboarding docs (tracked as task 006).

## Stop when you're done

```bash
docker rm -f xtr        # Path A
docker compose down     # Path B
```

Next: [Watch the automated tests pass](./automated-tests.md).
