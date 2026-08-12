# Fixture registry

This directory is the one place that answers three questions: what fixture
data exists, which files form a runnable scenario, and how a developer obtains
it. The catalog is tracked; large generated and captured bytes are not.

## Mental model

- A **package** is an immutable, checksummed archive stored outside Git. Keep it
  cohesive and reusable: one map region, or one weather-source capture.
- A **scenario** is everything needed for one simulator run. It names packages
  and resolves semantic inputs such as `map`, `gpx`, `weather`, and route dirs.
- A **profile** is a convenience set for a workflow. `sim`, `weather`, and
  `test` are the standard profiles.

The catalog at [`catalog.toml`](catalog.toml) is authoritative. Consumers must
not know bucket paths or reach into another crate's `assets/` directory.

## Daily use

```sh
obc fixtures list                 # scenarios, cache state, download size
obc fixtures show grimsel         # inputs, provenance, and packages
obc fixtures sync sim             # all simulator packages
obc fixtures sync test            # every external package used by tests
obc fixtures verify test          # re-hash archives and every extracted file
obc fixtures prune                # dry-run stale cache cleanup
obc fixtures prune --apply

obc sim grimsel                   # sync on first use, then run
obc sim monaco-upahead
```

The default cache is `~/.cache/openbikecomputer/fixtures`. Set
`OBC_FIXTURE_CACHE` to relocate it. Packages are addressed by SHA-256 and a
`by-id/` view gives tests stable logical paths. Downloads and extraction are
atomic; archives, manifests, and every extracted file are verified.
`tracked_sources` mappings additionally make CI fail if a small authored source
changes without rebuilding its external package.

Python 3.11 or newer is required (`tomllib` is part of the standard library).
There are no Python package dependencies.

## Adding or replacing data

1. Make a clean staging directory containing only the package's final layout.
2. Run `obc fixtures pack ID DIR --output ID.tar.gz`. Packing is deterministic.
3. Add or update the package entry with the printed byte count and SHA-256,
   then compose it into scenarios/profiles. Record source date, geography,
   transformation, and license in the catalog.
4. Run `obc fixtures publish ID ARCHIVE`. The maintainer-only command refuses
   bytes that differ from the catalog, performs an immutable R2 upload, sets
   long-lived cache metadata, and verifies the object through the public domain.
5. Run the registry tests and `obc fixtures sync/verify` from an empty cache.

For the two initial map packages, `fixtures/build-map-package.sh` preserves the
canonical bboxes and source URLs and writes its staging trees/archives under
`fixtures/build/`. Build Grimsel terrain before its map, or use the `all` target.

Never replace an object at an existing digest key. Never put fixture objects in
the production maps bucket: that bucket has an independent publication and
cleanup lifecycle. A larger Freiburg map is just another package; a scenario
can combine it with the existing historical weather package.

## What remains in Git

Small authored format vectors, parser corpora, and pixel goldens stay beside
their owning tests because code review needs their exact byte changes. Product
demo assets stay with the app that ships them. Shared authored inputs live in
`fixtures/sources/` and are also packed into their scenarios. Large maps,
terrain, provider captures, and realistic ride bundles belong in R2. Generated design-review
screenshots belong in PRs or project documentation, not a runtime asset folder.

## Initial package provenance

- `sim-grimsel`: the v13 OBCM described by the former simulator-assets log,
  packed from the 2026-08-08 Switzerland snapshot; its OBCT terrain is derived
  from Copernicus GLO-30 tile `N46_00_E008_00`. The GPX/OBCR/OBT inputs are
  project-authored.
- `sim-monaco`: the v13 OBCM from the 2026-08-08 Monaco snapshot plus the
  project-authored up-ahead GPX.
- `weather-dwd-icon`: the exact DWD captures formerly documented under
  `host/obc-wx-bake/tests/fixtures`.
- `weather-noaa`: the exact NOAA captures formerly documented in that same
  directory. Source files are preserved byte-for-byte inside the archive; the
  current revision contains both the original APCP spans and the corrected
  point-valid PRATE spans so the source-semantics PR can land independently.
- `weather-event-derecho` and `weather-event-airmass`: complete, reproducible
  MRMS/HRRR event packs. Their small `event.json` manifests remain tracked and
  are matched byte-for-byte through `tracked_sources`; upstream, baked, and
  truth bytes live only in the immutable packages.

## Storage contract

`fixtures.openbikecomputer.com` is a read-only custom domain for a separate R2
EU-jurisdiction R2 bucket, `obc-dev-fixture`. Developers and CI need no cloud credentials. Upload credentials are
maintainer-only and are intentionally not shared with the production map
publisher. R2 Standard storage is appropriate because these packages are small,
occasionally replaced, and downloaded interactively.

The publisher reads `OBC_FIXTURE_R2_BUCKET`, `OBC_FIXTURE_R2_ENDPOINT`,
`OBC_FIXTURE_R2_ACCESS_KEY_ID`, and `OBC_FIXTURE_R2_SECRET_ACCESS_KEY` from the
gitignored `tools/obc.local`. The endpoint must include `.eu.` for this bucket.
