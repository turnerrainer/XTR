# 014 — DSL loader scale optimization

## Filed

2026-07-29 — surfaced when harvesting all 637 Estonian X-Road
WSDLs from the RIA catalog and trying to boot XTR against the
~3000+ generated DSLs.

## Severity

**Medium**. Blocks the "ingest everything" workflow for large
catalogs. Small-to-medium harvests (a few dozen subsystems,
~500 endpoints) boot fine.

## Motivation

Task 013 works end-to-end at Ariregister scale (33 endpoints)
and at moderate scale (few hundred endpoints boot in a few
seconds). At full Estonian-catalog scale (~3000+ DSLs, harvested
via `scripts/harvest-xtee-wsdls.sh`), the DSL loader exceeds
5 minutes without completing.

Symptom: after successful ingestion (~30 seconds for 637 WSDLs),
`loader::load_all` starts reading the ~3000 generated `.yml`
files, parsing YAML, and running startup Handlebars validation
per template. Never logs "loaded N DSL(s)"; boot is effectively
hung.

Likely culprits (unprofiled):

- Handlebars template compilation is O(template-size) but not
  O(1). A ~10 KB envelope with many `{{...}}` placeholders takes
  measurable milliseconds to compile. At 3000+ templates, this
  adds up.
- Sequential file I/O — each DSL loaded one at a time.
- Full ServiceMap built in-memory before the router mounts.

## Fix options (not decided)

1. **Parallel loader**: `rayon` or `tokio::spawn`-per-file.
   Trivial for pure YAML parse; Handlebars compile can also
   run concurrently. Likely 4-8× speedup on multi-core, might
   drop boot to ~1 minute at 3000 scale.

2. **Lazy Handlebars validation**: skip at boot, validate on
   first request per DSL. Removes startup-time cost but
   sacrifices the task 013 v1 feature "bad template blows up
   at boot, not on first live request." Tradeoff.

3. **Async router mount**: bind the port + serve /health as
   soon as the executor is ready; load DSLs in the background,
   populate ServiceMap incrementally. `/api` reflects what's
   loaded so far. Endpoints 404 until their DSL loads. Very
   fast time-to-first-request; complex to implement correctly.

4. **Loader cache**: hash each `.yml` → cache compiled
   Handlebars template on disk under `<dsl_path>/.cache/`.
   Second boot reads pre-compiled. First boot still slow.

5. **Cap what we load**: reject DSLs whose envelope exceeds N
   KB. Combined with the type-ref node cap in task 013 v6,
   this bounds worst-case per-DSL loader cost. Fewer endpoints,
   faster boot.

Most likely combo for v1: option 1 (parallel loader) +
option 5 (envelope size cap in the generator).

## Acceptance

- `scripts/harvest-xtee-wsdls.sh` (full harvest) → XTR boots in
  under 60 seconds against the resulting ~3000 DSLs.
- Existing tests pass unchanged.
- If option 3 lands, `/health` responds within 1 second of
  process start regardless of DSL count.

## Estimated effort

- Profiling to identify actual bottleneck: half day.
- Depending on findings, likely 1-2 days for parallel loader
  or 2-3 days for background-load pattern.

## Dependencies

- Task 013 is landed; this is a scale follow-up.

## Non-scope

- Sharding XTR across multiple processes / multi-tenant setups.
- Per-endpoint auth / rate limiting under load (separate
  operational-hardening epic).
