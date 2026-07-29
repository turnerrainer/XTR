# Shipped WSDLs

XTR's default `xtr.yaml` sets `wsdl_watch_dir: ./wsdl`, so every
`<group>/*.wsdl` file below becomes live REST endpoints on boot
(task 013 folder-drop). Each `.wsdl` has a companion
`.meta.yaml` sidecar with `member_class` / `member_code` /
`subsystem_code` that triggers XTR's X-Road envelope wrapping.

## What ships

| Group prefix | Origin | Notes |
|---|---|---|
| `ar/` | Ariregister (RIK, GOV/70000310) — the Estonian Business Register public XML API. 33 auto-generated endpoints. Runs against `ariregxmlv6.rik.ee` directly (public HTTPS, no Security Server required — vendor username/password in the SOAP body). |
| `ads/`, `ehr3/`, `mis/`, `kpois/`, `kkrmn/`, `knr/`, `maaok/`, `maais/`, `maais-klient/`, `etak/`, `tiheasustusalad/`, `aks-ads/`, `aks-knr/`, `mtp-katri/` | **Maa-amet** (Land Board, GOV/70003098) — under Ministry of Climate since 2023 reorg. Address system, buildings register, topographic maps, land-cadastre operations. |
| `kotkas-70008658/`, `okas/` | **RMK** — State Forest Management Centre (GOV/70008658). Forest permits + operations. |
| `kotkas-70009445/`, `metsaregister/`, `DHX/`, `DHX.kotkas/` | **Keskkonnaamet** — Environmental Board (GOV/70009445). Environmental permits + forest register. |
| `ljvis/` | **Keskkonnaministeerium / Kliimaministeerium** (Ministry of Climate, GOV/70001231) — waste tracking system. |

## Reproducing / expanding

Everything under this directory (except `ar/`, which is a
special-case public-HTTPS service) came from RIA's public
X-Road catalog via `scripts/harvest-xtee-wsdls.sh`. To add
more subsystems:

```bash
./scripts/harvest-xtee-wsdls.sh --member <memberCode>[,memberCode...]
./scripts/harvest-xtee-wsdls.sh --subsystem <subsystemCode>[,subsystemCode...]
```

For the full RIA catalog (~421 subsystems, ~4300 methods),
run without flags — but the DSL loader currently stalls at
that scale. See task 014.

## Security Server requirement

Every group EXCEPT `ar/` needs a configured `security_server:`
in `xtr.yaml` to actually work — the sidecars produce
X-Road-wrapped envelopes that route via mTLS through YOUR
Security Server. Without SS, `/api` lists all endpoints but
every call returns "no security_server configured". See
`book/src/ops/xroad-security-server.md`.

`ar/` is the exception because Ariregister publishes a public
HTTPS fallback endpoint — the generator's TURVASERVER
detection replaces the placeholder with the vendor URL for
these DSLs. Vendor username/password in the SOAP body is
still required.
