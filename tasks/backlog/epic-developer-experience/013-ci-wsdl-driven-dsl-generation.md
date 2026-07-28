# 013 — CI-driven WSDL → DSL generation (pre-generated, tested, published)

## Filed

2026-07-28 — filed initially as a CLI scaffold generator; revised
same-day twice per Rainer's directives:
  * v1 (dropped): one-shot CLI scaffold.
  * v2 (dropped): runtime auto-generation at boot.
  * v3 (this): build-time (CI) generation. WSDL → DSL happens
    in CI, generated `.yml` files land under `DSL/<group>/*.yml`,
    are tested automatically, and published as part of the
    canonical repo + release artifact. XTR runtime doesn't
    know or care about WSDLs — it still just loads `.yml` files
    from disk.

## Severity

**High** for adoption. Runtime unchanged means zero
architectural risk to XTR's proven request path. Everything
new lives in CI + tooling. The output is version-controlled,
reviewable in PRs, and trivially rollback-able.

## Design

### 1. Source-of-truth manifest

New repo-root file `wsdl-sources.yaml`:

```yaml
# The canonical list of WSDLs XTR wraps.
# CI regenerates DSL/<group>/*.yml from these on every push.
sources:
  - group: ar
    wsdl: https://ariregxmlv6.rik.ee/?wsdl
    # Optional: filter which operations to expose. Absent = all.
    operations:
      - lihtandmed_v3
      - detailandmed_v2
      - ettevottegaSeotudIsikud_v1
      - tegelikudKasusaajad_v2
    # For X-Road-target sources, add:
    # xroad_target:
    #   member_class: COM
    #   member_code: "12345678"
    #   subsystem_code: some-subsystem

  - group: xroad
    # Meta-services shipped with X-Road itself. WSDL URL is
    # part of the X-Road standard.
    wsdl: <upstream-catalog-URL-TBD>
    operations: [listMethods, allowedMethods]
```

### 2. The generator

New binary or subcommand: `xtr-gen-dsl` (separate crate under
`tools/` so it never bloats the runtime binary).

Input: `wsdl-sources.yaml` + a target output directory.
Output: `DSL/<group>/<operation>.yml` per operation across
all sources.

Deterministic output — same WSDL input always produces the
same YAML bytes so drift detection works. That means:
  * Stable YAML ordering (fields in the same order every
    time).
  * Stable whitespace (single-space indent, LF line endings,
    trailing newline).
  * Reproducible envelope generation regardless of
    HashMap-iteration order.

### 3. CI job (`.github/workflows/regen-dsl.yml`)

Runs on `push` to any branch under `dev`:

```yaml
jobs:
  regenerate:
    steps:
      - checkout
      - cargo build tools/xtr-gen-dsl
      - cargo run -p xtr-gen-dsl -- \
          --sources wsdl-sources.yaml \
          --out DSL/
      - git diff --exit-code DSL/     # drift detection
        # if diff is non-empty → CI fails; author must
        # commit the regenerated DSL files
```

Alternative: an auto-commit workflow that regenerates + opens
a PR when a WSDL changes upstream. Nice-to-have; drift-detect
is the MVP.

### 4. Testing the generated DSLs

CI runs (in this order):

  a. **Structural test** — every generated `.yml` parses via
     `dsl::loader::load_all`. Failure here means the
     generator is broken.
  b. **Handlebars validation** — reuses existing
     `validate_template` from `dsl/loader.rs` — envelope must
     compile as a Handlebars template.
  c. **Auto-context test** — with a fixture config, expand
     each envelope using the current `expand()` and assert
     the output is well-formed XML with all placeholders
     resolved.
  d. **Round-trip test against a captured WSDL fixture** —
     for each generator input, `tests/fixtures/wsdl/<vendor>.wsdl`
     is a captured copy of the vendor's WSDL. Running the
     generator against the fixture must produce byte-equal
     `.yml` files to what's committed. This is what makes
     drift detection meaningful.
  e. **Live smoke test** — one operation per source is called
     against a mock upstream (or, opt-in via env var, the
     real upstream) and the response is asserted well-formed.
     Uses task 007's mock SS harness once landed; ad-hoc
     `axum` upstream in the meantime.

### 5. Publishing

Generated DSLs are committed to the repo — they ARE the
canonical shipped content. The container image `COPY`s the
`DSL/` tree at build time so the published Docker image ships
with them baked in.

For consumers who want their own DSLs (not vendor-provided),
they mount over `/app/DSL/` at container run time — same as
today.

### 6. Handling WSDL evolution

When a vendor changes their WSDL:
  * CI's next scheduled run notices drift (job runs daily
    via `schedule: cron` + on push).
  * Drift-detect fails the run.
  * Operator fetches the new WSDL locally, regenerates,
    reviews the diff (often trivial — a new optional field,
    a new operation), commits.
  * Version bump: the release cycle picks up new DSLs on the
    next tag.

For breaking WSDL changes (removed field, renamed operation):
  * Drift-detect catches it.
  * Operator decides: accept the break and bump version, or
    override with a hand-written DSL that pins the old
    envelope. Task 013's design keeps hand-written DSLs in
    the same `DSL/<group>/<service>.yml` slot; if a
    hand-written file already exists, generator SKIPS
    (doesn't overwrite) and logs.

## Acceptance

- `wsdl-sources.yaml` exists and captures the current 6
  shipped DSLs (4 Ariregister + 2 X-Road meta).
- `tools/xtr-gen-dsl` builds standalone and produces
  byte-equal output to the committed `DSL/` on first run
  (i.e., the current hand-written DSLs are locked in as the
  target).
- CI job runs generation + drift-detect + structural +
  Handlebars + expansion + round-trip tests on every push.
- CI job runs daily via cron to catch upstream WSDL changes.
- New `book/src/ops/wsdl-sources.md` chapter documents the
  manifest format, how to add a new source, and the drift-
  detection failure mode.
- Existing `book/src/dsl/adding-a-service.md` rewritten:
  auto-generation (via `wsdl-sources.yaml`) becomes the
  primary path; hand-writing a DSL becomes the override
  fallback for WSDL-less services or vendor-bug workarounds.
- XTR runtime unchanged. Every existing test passes.
  `src/`  gains zero new code.

## Estimated effort

- Minimal WSDL parser (SOAP 1.1 subset X-Road uses) as a
  `tools/xtr-gen-dsl` crate: 2 days
- Deterministic YAML emission: half day
- CI workflow (regen-dsl.yml) with drift detection + test
  gates: half day
- Round-trip fixtures for the 6 shipped DSLs: half day
- Auto-context injection for X-Road-target sources: half
  day
- Docs (`ops/wsdl-sources.md` + rewritten
  `dsl/adding-a-service.md`): 1 day

Total: **~4.5 days** — smaller than the runtime-auto-gen
version because zero refresh/cache/atomic-swap machinery.

## Dependencies

- Task 007 (mock SS harness) helps the round-trip test but
  isn't required.

## Non-scope but adjacent

- **Runtime WSDL fetching**. Deliberate — v3 of this task
  rejected it in favor of deterministic committed output.
- **CLI scaffold for one-off use** (`xtr scaffold`). If an
  operator wants to generate a single DSL from a WSDL they
  don't want in `wsdl-sources.yaml`, that's a follow-up task
  — the machinery from this task makes it trivial.
- **WSDL 2.0**. Not used in X-Road.
- **Auto-opening PRs when WSDL drifts**. Drift-detect fails
  CI; the operator regenerates + commits. Auto-PR is a
  refinement.
- **REST client codegen from `/api`**. Standard OpenAPI
  tooling downstream.

## Risks

- **Generator determinism bugs**. If ordering isn't stable,
  drift-detect false-positives on every run. Mitigation:
  round-trip test asserts byte-equal output twice in a row
  in CI itself.
- **Vendor WSDL that uses constructs we don't support**.
  Bail with clear error naming the construct; don't emit a
  wrong DSL. Operator either overrides with hand-written or
  fixes the generator.
- **Hand-written override precedence isn't obvious**.
  Document loudly in both `ops/wsdl-sources.md` and
  `dsl/adding-a-service.md`. Generator prints a WARN when
  it skips a file because a hand-written override exists.
