# Shipped WSDLs — owner-grouped

XTR's default `xtr.yaml` sets `wsdl_watch_dir: ./wsdl`, so every
`*.wsdl` file below becomes live REST endpoints on boot
(task 013 folder-drop). Each `.wsdl` has a companion
`.meta.yaml` sidecar for X-Road envelope wrapping.

## Layout

```
wsdl/
├── <owner>/                    # organisation slug — becomes URL prefix
│   └── <subsystem>/            # X-Road subsystem code — organisational only
│       ├── N.wsdl              # one WSDL per operation-set
│       └── N.meta.yaml         # sidecar: member_class/code/subsystem
```

Boots as `POST /<owner>/<subsystem>-<operation>` — the pipeline
collapses `wsdl/<owner>/<subsystem>/N.wsdl` into
`DSL/<owner>/<subsystem>-<op>.yml` so the URL is 2-segment and
per-owner listing groups everything from one ministry/agency.

## What ships

| Owner slug | Organisation | Subsystems |
|---|---|---|
| `rik/` | RIK — Registrite ja Infosysteemide Keskus (Centre of Registers and Information Systems) | `ariregister` (public HTTPS demo, no Security Server needed — see sidecar `service_url` override) |
| `maa-amet/` | Maa-amet (Land Board — under Kliimaministeerium since 2023) | `ads`, `ehr3`, `mis`, `kpois`, `kkrmn`, `knr`, `maaok`, `maais`, `maais-klient`, `etak`, `tiheasustusalad`, `aks-ads`, `aks-knr`, `mtp-katri` |
| `keskkonnaamet/` | Keskkonnaamet (Environmental Board) | `kotkas`, `metsaregister`, `DHX`, `DHX.kotkas` |
| `rmk/` | RMK (State Forest Management Centre) | `kotkas`, `okas` |
| `kliima/` | Kliimaministeerium (Ministry of Climate, ex-Keskkonnaministeerium) | `ljvis` |

## Reproducing / expanding

Everything under `wsdl/` (except `rik/ariregister/`, which is the
public HTTPS special case) came from RIA's public X-Road catalog
via `scripts/harvest-xtee-wsdls.sh`:

```bash
./scripts/harvest-xtee-wsdls.sh --member <memberCode>
./scripts/harvest-xtee-wsdls.sh --subsystem <subsystemCode>
```

The script's built-in ownership map (memberCode → slug) puts
new fetches under `wsdl/<owner>/<subsystem>/`. Add more mappings
in the script's `OWNERS` dict for new memberCodes.

## Security Server requirement

Every group EXCEPT `rik/ariregister/` needs a configured
`security_server:` in `xtr.yaml` to actually work — the sidecars
produce X-Road-wrapped envelopes that route via mTLS through
YOUR Security Server. Without SS, `/api` lists all endpoints but
every call returns "no security_server configured".

`ariregister` is the exception because its sidecar sets
`service_url: https://ariregxmlv6.rik.ee/` — direct-HTTPS
fallback endpoint the vendor publishes alongside their X-Road
subsystem. Vendor username/password in the SOAP body still
required.
