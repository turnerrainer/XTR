# 015 — Separate vendored-WSDL catalog into its own repository

## Filed

2026-07-29 — Rainer during the "harvest all EE X-Road WSDLs"
experiment: "We could store these DSLs separately out of XTR
itself."

## Severity

**Low / when-needed**. XTR ships with `wsdl/ar/` (Ariregister,
~180 KB) as its default demo. The harvest script
`scripts/harvest-xtee-wsdls.sh` fetches the full RIA catalog
(~53 MB of WSDLs across ~421 subsystems) on demand. That
model works today; committing the full harvest to XTR itself
would bloat the repo without corresponding runtime benefit.

But if an operator wants version-controlled catalog data —
notice when a vendor's WSDL changes, roll back to a prior
version, share a curated subset with a team — the natural
shape is a separate repository.

## Motivation

Two properties of the vendored catalog data that don't
belong in XTR proper:

- **Domain-specific**: XTR is a generic REST-to-SOAP proxy;
  the Estonian catalog is one specific ecosystem's data. A
  Finnish operator wouldn't want EE WSDLs in their XTR
  install; a NL operator would want theirs. Shipping any
  specific catalog with XTR privileges one ecosystem.
- **Different release cadence**: XTR's version tracks its
  own feature/fix cycle; a catalog repo's version tracks
  upstream WSDL drift (which is out of the operator's
  control). Coupling them forces XTR releases every time a
  vendor updates their WSDL.

## Shape

New repository (proposed name): `xtr-catalog-ee`.

Structure:

```
xtr-catalog-ee/
├── README.md              # what this is, refresh cadence, licensing
├── LICENSE                # matches the WSDL vendors' terms
├── wsdl/                  # organised same as XTR's wsdl_watch_dir
│   ├── ar/
│   │   ├── ariregister.wsdl
│   │   └── ariregister.meta.yaml
│   ├── rr/
│   │   └── ...
│   └── ...
├── scripts/
│   └── refresh.sh         # re-runs XTR's harvest script and
│                          # diffs against committed state.
│                          # CI-friendly.
└── .github/workflows/
    └── refresh.yml        # scheduled cron; opens PR on drift.
```

Operator wires it up either by:

- **Git submodule**: `git submodule add
  https://github.com/turnerrainer/xtr-catalog-ee wsdl` — XTR
  picks it up under its default `wsdl_watch_dir: ./wsdl`.
- **Volume mount**: `docker run -v
  /host/xtr-catalog-ee/wsdl:/app/wsdl xtr-on-rust:...` —
  container consumes the catalog dir directly.
- **CI-fetched artifact**: pull a tarball from
  `github.com/.../releases/latest/xtr-catalog-ee.tar.gz` into
  the operator's Docker build.

XTR itself doesn't change — this is entirely a repo-topology
concern.

## Acceptance

- New repo exists at `github.com/turnerrainer/xtr-catalog-ee`
  or similar.
- `wsdl/` sub-tree matches the structure XTR's
  `scripts/harvest-xtee-wsdls.sh` produces.
- Weekly cron re-harvests + opens PR when upstream WSDLs
  drift.
- XTR's docs (`book/src/ops/wsdl-ingestion.md`) mention the
  separate catalog repo as the preferred production shape;
  the in-repo harvest script stays as the "one-shot,
  no-commit-history" alternative.
- Retain XTR's shipped `wsdl/ar/` as the built-in demo
  (Ariregister remains the small, illustrative default;
  the catalog repo is opt-in for the full ecosystem).

## Estimated effort

- New repo scaffold + first harvest + CI cron: half a day.
- XTR docs pointer: quarter day.
- Optionally: a second repo (`xtr-catalog-fi` etc) for
  Finnish / Icelandic / other X-Road instances. Same shape,
  different catalog source URL.

## Non-scope

- Auto-updating XTR itself from the catalog repo (that's
  the operator's plumbing — submodule / volume mount /
  release artifact).
- Consolidating multiple national catalogs into one repo.
  Per-country repos keep licensing/authorship clean.
- Building a searchable catalog frontend. That's what
  RIA's x-tee.ee already is; no reason to duplicate.

## Dependencies

- Task 013 landed (WSDL folder-drop feature).
- `scripts/harvest-xtee-wsdls.sh` landed (fetch primitive).
