# X-Road Security Server setup

Skip this chapter if you only need the shipped Ariregister demo —
that hits `ariregxmlv6.rik.ee` directly, no Security Server needed.

Read on when you need any of the other 160+ shipped endpoints
(Maa-amet, Keskkonnaamet, RMK, Kliimaministeerium), or any real
X-Road service in general.

## What a Security Server is

A Debian-based appliance published by
[NIIS](https://github.com/nordic-institute/X-Road). It sits on the
border between your organisation and the X-Road network:

- Terminates mTLS on both sides (your client cert → SS → peer cert)
- Wraps outbound requests in the standard X-Road envelope
- Validates response `requestHash` — proves the response is genuinely
  a reply to your request
- Logs everything for audit
- Registers your organisation's identity with the central authority

XTR itself does none of these — it just points at your Security
Server over mTLS.

## Decision tree

| Your goal | You need |
|---|---|
| Call Ariregister (shipped demo) | Nothing — works out of the box |
| Any other X-Road test service | An `ee-test` Security Server (~half a day, free) |
| Real Estonian production service | Full RIA onboarding, contracts, paid CA cert (weeks) |

Rest of this chapter is the `ee-test` middle row.

## Prerequisites

- Public-IP Linux VM (Ubuntu 22.04 or Debian 12, ~2 vCPU / 4 GB RAM)
- DNS name pointing at the VM
- Firewall: inbound 4000 (admin UI), 5500–5501 (message exchange),
  5577 (OCSP proxy), 22 (SSH); outbound 80/443 to central authority
  + your target services
- A subsystem name (anything for test, matches your registration for
  prod)
- A free RIA test account

## Install & register (map, not commands)

Full commands live in the [NIIS X-Road manuals](https://github.com/nordic-institute/X-Road/tree/develop/doc/Manuals) — package
names and admin-UI wording drift between releases, so consult the
current version. The stable shape:

1. Add NIIS APT repo (country-specific: `ubuntu-22.04-current-ee`
   for `ee-test`).
2. `apt install xroad-securityserver-ee` — interactive wizard
   sets admin UI address + initial password.
3. Log in to the admin UI at `https://<vm-fqdn>:4000/`.
4. Import RIA's `ee-test` configuration anchor via the admin UI.
5. Generate a signing keypair + CSR, paste CSR into RIA's
   self-service portal, upload the returned cert.
6. Register your subsystem (`ee-test` auto-approves in minutes).
7. Verify: admin UI's Client tab shows your subsystem as
   `Registered` (green).

## Export the PKCS12 for XTR

XTR needs an **information-system client key** (not the SS's own
signing key):

1. Admin UI → Keys and Certificates → generate a software-token
   key with usage `sign` + `auth`.
2. Export as PKCS12 with a strong password.
3. Move `.p12` onto the host running XTR (or bake into your image).

**Never commit `.p12` files or passwords.** Password comes to XTR
via `XTR_KEYSTORE_PASSWORD` env var.

## Wire XTR

```yaml
# xtr.yaml
xroad_instance: ee-test
client_data:
  member_class: GOV
  member_code: "70000000"                 # your org registry code
  subsystem_code: <your-subsystem>        # what you registered
security_server:
  url: "https://<your-ss-fqdn>:5500/"     # YOUR SS, not the central authority's
  keystore_path: /app/ssl/xtr-client.p12
  keystore_password_env: XTR_KEYSTORE_PASSWORD
```

Run:

```bash
XTR_KEYSTORE_PASSWORD='<the-p12-password>' \
  docker run -d --name xtr -p 8080:8080 \
    -v $PWD/xtr.yaml:/app/xtr.yaml:ro \
    -v $PWD/ssl:/app/ssl:ro \
    -e XTR_KEYSTORE_PASSWORD \
    turnerrainer/xtr:rc
```

Live-verify with the shipped X-Road meta-service (needs a real
target subsystem to query):

```bash
curl -sX POST http://localhost:8080/xroad/listMethods \
  -H 'content-type: application/json' \
  -d '{"member_class":"GOV","member_code":"70000000","subsystem_code":"target-subsystem"}'
```

## Failure modes specific to mTLS

| Symptom | Cause |
|---|---|
| `keystore_load_failed: parsing PKCS12` at startup | Wrong password, wrong file, or the `.p12` was generated with a modern (AES) cipher OpenSSL rejects. Regenerate with `-legacy` or RC2/3DES on export. |
| Every call `502 upstream_http_error` HTTP 401/403 | Your subsystem isn't authorized for that service. Ask the target's owner to add you to their allow-list. |
| Every call `504 upstream_timeout` | Firewall — outbound 5500 to your SS's peers is blocked. |
| `SSL routines::wrong version number` in XTR logs | `security_server.url` port is wrong — should be 5500 (message port), not 4000 (admin UI). |
| Subsystem stuck in `GLOBALERROR` in admin UI | Registration hasn't propagated. `ee-test`: wait 15 min. Prod: contact RIA. |

## Production heads-up

`ee-test` self-service does **not** scale to production. Prod requires:

- Signed contract with RIA
- Operationally-hardened SS (backups, monitoring, cert rotation)
- Paid CA cert (not the free test cert)
- Formal legal registration of the org + subsystem

Ballpark: several weeks calendar time, single-digit thousands EUR
setup. Don't plan production integration on `ee-test` timelines.
