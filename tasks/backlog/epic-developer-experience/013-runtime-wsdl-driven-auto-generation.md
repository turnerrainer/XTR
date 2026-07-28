# 013 — Runtime WSDL-driven DSL auto-generation

## Filed

2026-07-28 — filed initially as a CLI scaffold generator; revised
same-day to runtime auto-generation per Rainer's directive
("XTR DSLs should be generated based on WSDLs by XTR
automatically"). CLI scaffold survives as a fallback tool
described in "Non-scope but adjacent" below.

## Severity

**High** for real-world adoption. Every service integration today
starts with a human hand-writing a SOAP envelope from a WSDL —
friction that scales linearly with the number of services
wrapped. Making WSDLs the source of truth flips XTR from
"YAML-DSL-driven proxy" to "point-and-consume X-Road service
gateway."

## Motivation

Current model: DSL YAML is the source of truth. Every operation
you want to expose requires an author to:
1. Read the target's WSDL,
2. Hand-write a Handlebars envelope with correct namespaces,
3. Declare the `params:` allow-list to match the WSDL's input
   message,
4. Restart, curl, iterate.

Machine-readable input (WSDL) → human-hand-transcribed output
(YAML) is pure toil, and drift between the two only shows up as
`Client.MissingParam` faults at request time.

**Target model**: WSDL is the source of truth. XTR fetches
declared WSDL sources at boot, parses each operation, and
materialises a live endpoint per operation with the envelope,
params list, and target URL derived automatically. Zero
hand-written YAML for services covered by a WSDL.

## Design

### Config

New `xtr.yaml` section:

```yaml
wsdl_sources:
  # Public HTTPS SOAP — direct fetch.
  - group: ar
    wsdl: https://ariregxmlv6.rik.ee/?wsdl
    # Optional: filter which operations to expose. Absent = all.
    operations: [lihtandmed_v3, detailandmed_v2,
                 ettevottegaSeotudIsikud_v1, tegelikudKasusaajad_v2]

  # X-Road subsystem — WSDL fetched through the configured SS.
  - group: eesti-post
    xroad_target:
      member_class: COM
      member_code: "10328093"
      subsystem_code: eesti-post-svc
    # WSDL URL from x-tee catalog, resolvable through SS.
    wsdl: "https://ss.example.org/wsdl/eesti-post-svc"
    operations: [trackParcel, estimateDelivery]

# Refresh policy — TTL for cached WSDL, or "boot-only".
wsdl_refresh_seconds: 3600      # 0 or absent = boot-only
wsdl_cache_dir: ./.xtr-wsdl-cache
```

### Startup flow

1. **Fetch each WSDL** (parallel, timeout-capped). Failures don't
   kill startup — they're logged as WARN and that source's
   operations are skipped. XTR still boots with whatever
   succeeded + whatever hand-written DSLs exist under
   `dsl_path:`.
2. **Parse WSDL → in-memory `XRoadTemplate`** per operation.
   Namespaces, element ordering, input message fields all lifted
   from the WSDL. Auto-context (`{{{generate.client}}}`,
   `{{generate.uuid}}`, etc) injected into the envelope for
   X-Road-target sources.
3. **Merge with hand-written DSLs**. On collision (auto-gen and
   hand-written both claim `POST /group/service`), **hand-written
   wins** — with a WARN log — so operators can override any
   generated envelope with a hand-tuned version.
4. **Cache the parsed WSDL** on disk under `wsdl_cache_dir` so
   repeated boots don't re-fetch (unless `wsdl_refresh_seconds`
   says otherwise).

### Runtime behaviour

Once loaded, auto-generated DSLs are indistinguishable from
hand-written ones: same request path, same allow-list filtering,
same Handlebars expansion, same executor, same response
translation. Every existing test + hardening layer applies
transparently.

`/api` (OpenAPI) lists auto-generated operations alongside
hand-written ones. The `operationId` gets a `_wsdl` suffix so
consumers can distinguish provenance if they care.

`/health` shape unchanged. Optional: a `GET /api/wsdl-sources`
endpoint reports which WSDLs were fetched, when, and how many
operations each yielded — useful for operators to spot silent
WSDL drift.

### Refresh + reload

If `wsdl_refresh_seconds > 0`, a background tokio task
re-fetches on schedule. A successful re-parse swaps the
in-memory templates atomically. A failed re-fetch keeps the
previous templates — never leaves XTR with a partially-loaded
service list.

Explicit trigger: `POST /admin/reload-wsdl` (auth-gated when
auth lands — filed separately as a DX concern for post-v1).

### What ships hand-written vs auto-generated

Post-landing, the shipped `DSL/samples/` tree can shrink
dramatically. Instead of 4 hand-written Ariregister DSLs, one
`wsdl_sources:` entry pointing at Ariregister's public WSDL
generates all 4 (and any new versions the vendor adds).
Hand-written DSLs remain for cases where:

- No WSDL exists (unlikely for X-Road; possible for legacy
  REST-fronted SOAP).
- The WSDL is wrong (envelope shape doesn't match reality) and
  the operator needs to override.
- The DSL needs custom Handlebars logic beyond what the WSDL
  encodes.

## Acceptance

- `xtr.yaml` `wsdl_sources:` section parses; missing/malformed →
  clear startup error naming the offending source.
- Round-trip test: point a fresh XTR at Ariregister's public
  WSDL; the 4 auto-generated `/ar/*` endpoints behave
  byte-equivalently (modulo whitespace differences in the
  envelope) to the current hand-written samples. Existing
  integration tests continue to pass against the auto-generated
  versions.
- WSDL fetch failures don't crash startup — logged as WARN,
  operator can still use hand-written DSLs.
- Auto-generated envelopes correctly handle:
  - Nested `xsd:complexType` (recurse into child elements).
  - Optional (`minOccurs=0`) fields — included in params list
    but marked as optional in OpenAPI.
  - `xsd:choice` — emit all alternatives, log a WARN suggesting
    the operator pick one via a hand-written DSL override.
- Hand-written DSL override wins on group/service collision,
  with a WARN log identifying the shadowed source.
- Cached WSDLs on disk survive restarts; refresh triggers proper
  re-parse.
- New `book/src/ops/wsdl-sources.md` chapter documents the
  config, cache behaviour, override semantics, and refresh
  policy.
- `book/src/dsl/adding-a-service.md` rewritten: auto-generation
  becomes the default path; hand-written path stays as the
  override fallback.

## Estimated effort

- WSDL parser (SOAP 1.1 subset X-Road uses): 2 days
- Config + fetch pipeline (parallel, timeout, WARN-on-fail): 1
  day
- Envelope generation with auto-context injection: 1 day
- Cache + refresh + atomic swap: 1 day
- Merge with hand-written DSLs (collision handling): half day
- Round-trip tests against Ariregister + a mock X-Road target
  (uses task 007's mock SS harness if landed, else ad-hoc
  axum test upstream): 1.5 days
- Docs: `wsdl-sources.md` ops chapter + revised
  "adding-a-service.md" walkthrough (auto path is the default;
  hand-written is the fallback for override cases): 1 day

Total: **~1 week** of focused work.

## Dependencies

- Task 007 (mock SS harness) would let the X-Road round-trip
  test run in CI without a real SS. Not strictly required but
  cheaper end-to-end coverage.
- No changes to core executor or router — pure additive.

## Non-scope but adjacent

- **CLI scaffold** (`xtr scaffold --wsdl <url> --operation <n>`)
  — originally what this task proposed. Nice as a one-shot
  "grab a DSL, hand-edit it, commit it" workflow when the
  operator wants a persisted YAML source. Landing runtime
  auto-gen first makes this redundant for most cases, but
  worth reviving as a follow-up task if operators ask for it.
- **WSDL 2.0 support**. Nobody in the X-Road ecosystem uses it.
- **Response-shape typing / codegen from WSDL response
  messages**. XTR's response is runtime-shaped from whatever
  the upstream returned; static typing not needed here.
- **REST client codegen**. That's a downstream `/api` OpenAPI
  concern — standard OpenAPI tooling handles it.
- **WSDL diff / breaking-change detection**. Nice-to-have,
  separate task.
- **Auto-registering XTR endpoints in a service catalog**.
  Separate concern.

## Risks

- **WSDL parsing surprises**. WSDLs in the wild vary hugely.
  Bail-out-on-unsupported-construct is the right posture —
  never silently emit a wrong envelope. Startup WARN + skip.
- **Vendor changes their WSDL, XTR endpoint shape changes**.
  This is the point of the feature, but it's also a
  compatibility hazard for API consumers. Recommended posture:
  pin `operations:` explicitly in `wsdl_sources:` and treat any
  removal from the WSDL as a startup WARN — operator can decide
  whether to remove the operation or override with
  hand-written.
- **Cold-start latency**. First boot fetches N WSDLs; slow
  networks hurt. Cache to disk mitigates; parallel fetch
  bounded by config-controlled concurrency.
