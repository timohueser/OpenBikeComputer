# RA01 — Capture versioned real data for Assistant development

Parent: #1734. Dependencies: none. This issue supplies inputs early; RA13 supplies the final
integrated simulator release. It does not wait for all feature code or generate substitute data.

## Deliverable and owners

Use `fixtures/catalog.toml`, `fixtures/README.md`, `fixtures/build-map-package.sh`, and existing
`obc-fixtures` commands. Add source captures, build recipes and authored GPS replays for:

1. Grimsel/Meiringen and a nearby Swiss mixed-surface road network with genuine alternatives.
   Reuse existing Swiss OSM and Copernicus sources where possible. The single Grimsel corridor
   may legitimately offer no easier alternative; do not manufacture one.
2. Monaco for dense POIs, paging and stable selection. Reuse the registered package.
3. West Cork around Dunlough Castle for historical content, multilingual names and walking-only
   versus bicycle access. Keep the extract regional, not all Ireland.

Capture OSM, terrain and Wiki inputs independently with retrieval/source dates, bounding polygon,
source URLs, licenses, exact revisions and checksums. Capture text and image bytes, not just a live
URL. Place large inputs in the existing content-addressed fixture store; keep small recipes,
manifests and authored traces in `fixtures/sources`. A clean worktree must not use files from
another local checkout or rely on `/tmp`/`docs/assets` as a production data source.

## Implementation requirements

- Pin raw source versions. Separate network acquisition from deterministic pack/build/replay.
  Add inputs to the existing registry and profiles; do not create another downloader daemon.
- List actual source POI/QIDs for the review cases with their coordinates and available facts.
  Identify unsupported/missing facts rather than editing real data to match expected outputs.
- Replays provide motion and reproducible clock/UTC-offset stamps through normal app ports.
  They must not inject open/closed flags, calculated costs, arrival events or selected routes.
- Include an out-and-back visit, forward rejoin, loop/crossing, dense list and a real landmark
  access example. Use crafted tiny contract fixtures separately for hard-to-source fault cases.
- Publish an initial source package before format changes; later children rebuild output maps
  through normal pack/cut/assemble paths and update immutable artifact revisions.
- Provide repeatable `obc fixtures sync/verify` and build commands. Full Swiss content statistics
  are produced by RA08/RA13; this issue does not claim regional inputs prove country coverage.

## Acceptance and verification

A clean cache can acquire and verify each manifest; a second offline build consumes the same bytes.
Every registered review example resolves to its recorded source identity. Checksum changes
invalidate the correct recipe. A missing source fails with a bounded actionable error. Run
fixture-registry/tool tests, `obc suites check`, and the affected fixture acquisition/build
commands. No application implementation, benchmark matrix, device flash or rendering sweep.
