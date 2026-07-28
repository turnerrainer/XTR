# Getting an X-Road Security Server

XTR-on-Rust is a client to an **X-Road Security Server (SS)**. If you
only need the plain-HTTPS path (Ariregister public XML, other public
SOAP endpoints), you can skip this chapter — those DSLs use `service:`
directly and no SS is involved. Read on when you need real X-Road
mTLS: `listMethods`, `allowedMethods`, or any registered
member-to-member service.

## What a Security Server actually is

An SS is a Debian-based appliance published by
[NIIS](https://github.com/nordic-institute/X-Road) (Nordic Institute
for Interoperability Solutions). It sits on the border between your
organisation and the X-Road network and does five things:

1. **Terminates mTLS** on both sides — clients present a PKCS12
   identity to your SS; your SS presents its own cert upstream.
2. **Wraps outbound requests** in the X-Road envelope
   (`<xroad:client>` / `<xroad:service>` / `<xroad:id>` /
   `<xroad:protocolVersion>`) and forwards.
3. **Validates responses** — checks the response's `requestHash`
   matches the outbound request. Any mismatch = tampering.
4. **Logs everything** for audit — every request/response with
   timestamps, sizes, signatures.
5. **Registers your identity** with the central authority (RIA for
   Estonia). Your `member-class / member-code / subsystem-code`
   triple only becomes valid X-Road identity after registration.

XTR-on-Rust talks to your SS over mTLS. XTR does 1-3 for the *client*
side of the mTLS conversation and lets the SS do the X-Road-specific
half.

## Decision tree

| Your goal | You need |
|---|---|
| Call Ariregister public XML | Nothing. Use `ar/*` DSLs directly. |
| Call a public X-Road test service via the mTLS path | An `ee-test` SS, self-service certs. Cost: ~€5–15/month VM. Time: half a day. |
| Call a real Estonian production service | Full RIA onboarding, real organisation registration, paid CA cert. Weeks. |
| Different country (Finland, Iceland, ...) | Different central authority; same SS software. |

The rest of this chapter covers the middle row: an `ee-test` SS you
own, connected to real (test-tier) X-Road services. That's the
useful development target.

## Prerequisites

| Item | Notes |
|---|---|
| Public-IP Linux VM | Ubuntu 22.04 LTS or Debian 12 recommended. 2 vCPU / 4 GB RAM is plenty for a test SS; production sizing depends on load. |
| Fully-qualified DNS name | Points at the VM. `ss-test.<your-domain>.tld` is a fine pattern. |
| Firewall access | Inbound: 4000 (admin UI), 5500-5501 (message exchange), 5577 (OCSP proxy), 22 (SSH). Outbound: 80/443 to the central authority + services you plan to call. |
| Shell access | Root (or sudo) on the VM. Everything is configured via `apt` + the admin UI. |
| A subsystem name | Your organisation's X-Road identity string. Pick anything for test — real strings must match your RIA registration. |
| An RIA test account | Self-service, free. Used to obtain test certs. |

## Installation walkthrough

The canonical, current-as-of-your-read-date instructions live in the
[NIIS X-Road installation
guide](https://github.com/nordic-institute/X-Road/tree/develop/doc/Manuals).
The high-level shape stays stable across versions:

1. **Add the NIIS APT repository** appropriate for your country
   (`ubuntu-22.04-current-ee` for `ee-test`, similar for other
   instances).
2. **Install the meta-package**: on Debian/Ubuntu roughly
   `apt install xroad-securityserver-ee`. The installer runs a
   short interactive wizard asking for the admin UI listen
   address and initial admin password.
3. **Log into the admin UI** at `https://<your-vm-fqdn>:4000/`.
4. **Import the configuration anchor** — a small XML file provided
   by RIA that tells your SS which central authority to trust. RIA
   publishes the current `ee-test` anchor on their portal.
5. **Provisional registration** — the SS generates a keypair, you
   submit a registration request through the admin UI, RIA (or the
   automated `ee-test` flow) approves it. For test-tier this is
   usually minutes.
6. **Obtain test certificates**:
   - Software token (default): the SS generates a signing key +
     CSR; you paste the CSR into RIA's self-service portal; the
     signed cert comes back; you upload it via the admin UI.
   - Alternative: hardware token (`opensc` + a PKCS11 device). Not
     needed for `ee-test`.
7. **Register a subsystem** — pick a `subsystem-code` (e.g.
   `test-subsystem`), request registration via the admin UI, wait
   for approval (automatic on `ee-test`).
8. **Verify** — the admin UI's "Client" tab should show your
   subsystem as `Registered` (green). Try
   `Security Server → Clients → your subsystem → Services → listMethods`
   against a known-open service to confirm end-to-end connectivity.

None of the numbered steps have hardcoded commands here on purpose —
NIIS revises apt package names and admin-UI wording between versions.
Follow the current NIIS manual for the exact commands; use this list
as the map so you know where you are.

## Producing the PKCS12 that XTR needs

Once your SS has an admin-approved subsystem, XTR does *not* need the
SS's internal signing key. XTR needs an **information-system client
key** that identifies YOUR calling app (XTR itself) to the SS.

1. In the SS admin UI: **Keys and Certificates → generate a
   software-token key** with usage `sign` + `auth` for an
   information-system client.
2. Export the resulting keypair as PKCS12 (`.p12`), with a strong
   password.
3. Move the `.p12` onto the host running XTR (or into your
   container's image if you build one).

**Never commit `.p12` files or their passwords.** See
[STANDARDS.md](https://github.com/turnerrainer/XTR/blob/dev/STANDARDS.md)
and [SECURITY.md](https://github.com/turnerrainer/XTR/blob/dev/SECURITY.md).
Password comes to XTR through the `XTR_KEYSTORE_PASSWORD` env var
(fixes JVM bug #16 — no default password baked in).

## Wiring XTR to the SS

Once the SS is running and you have the PKCS12, XTR needs three
things in its config:

```yaml
# xtr.yaml
xroad_instance: ee-test
xroad_protocol_version: "4.0"     # default; only bump when RIA does

client_data:
  member_class: GOV               # or COM, NGO, NEE per your RIA registration
  member_code: "70000000"         # your organisation's registry code
  subsystem_code: test-subsystem  # what you registered in step 7 above

security_server:
  url: "https://<your-ss-fqdn>:5500/"   # your SS, NOT the central
                                        # authority's SS
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD
```

Then run XTR with the password in the environment:

```bash
XTR_KEYSTORE_PASSWORD='<the-p12-password>' \
  ./xtr-on-rust --config ./xtr.yaml
```

Live-verify with one of the shipped X-Road DSLs:

```bash
curl -sX POST http://127.0.0.1:8080/xroad/listMethods \
  -H 'content-type: application/json' \
  -d '{"member_class":"GOV","member_code":"70000000","subsystem_code":"target-subsystem"}'
```

If the response comes back with a JSON `body` describing the target's
service list, the whole path — REST → SOAP wrap → mTLS to SS → X-Road
envelope to peer → response back — works.

## What can go wrong

| Symptom | Likely cause |
|---|---|
| XTR fails to start with `keystore load failed: parsing PKCS12 keystore` | Wrong password, wrong file, or the `.p12` was generated with a modern (AES) cipher that `native-tls`'s underlying OpenSSL rejects. Regenerate with `-legacy` (OpenSSL 3) or specify RC2/3DES on export. |
| XTR starts but every X-Road call returns `502 upstream_http_error` with a SOAP Fault | Your SS accepted the mTLS connection but the fault comes from the *target service*'s SS or the target service itself. XTR translates the fault to `upstream_soap_fault` when it can parse one — check the response body for the fault `code`. |
| Every call returns `502 upstream_http_error` HTTP 401/403 | Your subsystem isn't authorized to call this service. Ask the service owner to add your subsystem to their allow-list. |
| Every call returns `504 upstream_timeout` | Firewall — outbound 5500 to the SS's own peers is likely blocked. |
| SS admin UI shows subsystem in `GLOBALERROR` state | Registration hasn't propagated globally yet. On `ee-test`, wait 15 minutes and refresh. On production, contact RIA. |
| `SSL routines::wrong version number` in XTR logs | You're pointing `security_server.url` at the wrong port (probably 4000 = admin UI HTTPS-with-different-cert, not 5500 = X-Road message port). |

## Production heads-up

`ee-test` self-service does not translate to production. Production
X-Road membership requires:

- A **signed contract** with RIA (or your national X-Road authority).
- An **operational SS** that meets RIA's operational-security
  requirements — real backups, real monitoring, real cert-rotation
  procedures. Not a `t3.small` you spun up and forgot about.
- A **paid CA-issued cert** (not the free test cert).
- **Formal onboarding**: legal registration of the organisation +
  subsystem, RIA review, sometimes in-person kickoff.

Ballpark: several weeks calendar time, single-digit thousands EUR
setup + ongoing operational cost. Don't plan production integration
work assuming the `ee-test` flow scales; it doesn't.

## What XTR does NOT do

- **Manage SS keys.** XTR reads a `.p12` you produced via the SS
  admin UI. Rotation, expiry monitoring, HSM integration — all
  yours.
- **Register subsystems.** XTR uses whatever identity you configured;
  it does not talk to the central authority.
- **Verify response `requestHash`.** Task 004
  ([backlog](https://github.com/turnerrainer/XTR/blob/dev/tasks/backlog/epic-xroad-protocol-compliance/004-response-request-hash-verification.md))
  will add this; today XTR trusts the SS's `requestHash` semantics.
- **Log-signed request/response archival.** That happens in the SS,
  not in XTR.

For the SS itself, refer to
[NIIS documentation](https://github.com/nordic-institute/X-Road) and
your national authority (RIA for `ee-test` / `EE`).
