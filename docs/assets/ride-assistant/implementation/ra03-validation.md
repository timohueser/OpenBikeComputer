# RA03 implementation and validation

Implementation commit: `c8050c6d`; transform corrections: `1693c2a1`.
Issue: [RA03 #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738).

## Integration dependency

The branch is stacked on RA02 `e6f83c32`. RA02 supplies OBCM v15, the place
metadata extension, and rebuilt small map fixtures. RA03 uses edge count bit 15
for complete DEM integration; the graph record length stays unchanged. Production
source, normative nav/POI sections, shared vectors, and the terrain test fixture
merged cleanly. The public format table conflict is resolved as OBCM 15 / OBCR 4.
The normative version check is `0x0F` throughout.

Focused checks after stacking:

- `cargo test -p obc-route --test nav --test transform --test facts --test format`
  (71 pass)
- `cargo test -p obc-pack --test nav_round_trip -p obcm-assemble --lib`
  (321 pack unit, 20 graph contract, and 77 assembler unit tests pass)
- `cargo test -p obc-vectors --test vectors -p obc-web-assemble --test determinism`
- `python3 docs/build_docs.py --check-links`

The orchestrator owns the final Grimsel package rebuild after the RA09 header
extension. No large fixture package was rebuilt during this stack.

OBCR v4 has a 160-byte header, 7-byte point records, 80-byte waypoints, and an
optional 80-byte accepted-visit descriptor. The normative producer matrix is in
`specs/OBCR_Spec.md`. RA04/RA05 own the accepted-visit producer and durable route
checkpoint binding. Current trim/splice reject visit-bearing input. Planned-route
owners must call `NavPlanner::set_attribution_map` with the installed map identity;
otherwise measured surfaces remain unbound and cannot establish comparison eligibility.

## Production offline evidence

The actual simulator CLI imported the tracked Grimsel GPX with the installed map
from pinned `sim-grimsel` package
`a312858b96c89b11b2ab9ef0b59e943a4a771d942efcd55e38d0de9d3a4ce6ff`.
This exercises the production graph attribution hook, not a test-only converter.

```sh
cargo run -p obc-sim -- \
  ~/.cache/openbikecomputer/fixtures/by-id/sim-grimsel/grimsel.obcm \
  --import fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx \
  --routes-dir /tmp/ra03-attributed-routes
cargo run -p obc-vectors --example route -- \
  --inspect /tmp/ra03-attributed-routes/grimsel-climb.obcr
```

Reloaded facts: 18,679 m, +1,087 m, -0 m. Known elevation spans cover 18,679 m.
Surface distances are 13,922 m paved, 143 m cobbles, and 4,614 m unknown.
The real temporary card supplies object 1, revision 1, and a unique Store ID.
Reload and adjacent-half conservation of ascent, descent, and each surface class pass.
A new card has a different identity, so these facts do not become current-map facts
for that card automatically. Persistent-card imports use its persistent identity.

The old package is OBCM v14. Repeat final integrated acceptance on the RA02 v15
package at integrated acceptance; do not patch the old content-addressed package in place.

The authored Grimsel route was regenerated from its tracked original GPX. Both
versions have zero waypoints. Its original name `grimsel-climb` is preserved.
Plain-import totals are 18,673 m and +1,087 m. Surface attribution retains more
vertices at class boundaries, which explains the 6 m difference in stored distance.
The new authored OBCR SHA-256 is `bf45d5dc5836c3dafb700afe261636d31e1e6892d9fd64aacf41642057fea41c`.

## Checks completed

Whole suites, with follow-up runs only after changes or observed failures:

- `cargo test -p obc-route -p obc-formats -p obc-web-convert --no-fail-fast`
- `cargo test -p obc-pack --test nav_round_trip -p obcm-assemble --lib`
- `cargo test -p obc-sim --bin obc-sim -p obc-app --lib` (app: 992 pass;
  the simulator terrain fixture needed its complete-coverage bit)
- `cargo test -p obc-sim --bin obc-sim` after the fixture correction: 78 pass
- Final transform correction: `cargo test -p obc-route --test transform --test detour --test convert --test facts` (45 pass)
- `cargo test -p obc-vectors --test vectors -- --ignored` regenerated shared vectors
- `cargo clippy -p obc-route -p obc-pack -p obcm-assemble --all-targets -- -D warnings`
- `cargo clippy -p obc-route -p obc-app -p obc-sim --all-targets -- -D warnings`
- Final changed route/simulator delta: `cargo clippy -p obc-route -p obc-sim --all-targets -- -D warnings`
- `cargo check -p obc-route --lib --no-default-features`
- `swift test --package-path companion-ios/Packages/OBCKit --filter 'RouteObjectCodecTests|RouteStatsTests|RouteReverseTests|WaypointPlacementTests|LibraryStoreTests'`
- `swift test --package-path companion-ios/Packages/OBCKit --filter LibraryStoreTests`
  after adding non-default surface, coverage, and provenance to the persisted fixture
- `./tools/obc suites check` (71 suites, 355 execution units)
- `./tools/obc test affected --base origin/develop --dry-run`
- `cargo fmt --all` and `cargo fmt` for the board, boot, and desktop standalone roots
- `python3 docs/build_docs.py --check-links`
- `git diff --check`

Tests distinguish flat zero elevation, absent elevation, interior gaps, graph gaps
with valid emitted endpoints, clipped intervals, chunk seams, repeated/parallel
edge ambiguity, and malformed optional metadata. The decoded RoutePoint remains
12 bytes. Grades use ordered endpoint samples; unknown profile spans remain empty.

Transforms retain stored vertices and use the encoder's measured output distance for
waypoint shifts. A dense route at Swiss coordinates checks head and tail anchors
against the emitted geometry to the stored metre precision, including an exact
route-end waypoint. Interpolation adds the rounded local coordinate delta to the
integer anchor, so it does not round absolute coordinates through `f32`.
A segment marked incomplete returns unknown elevation at an interior seek, clipped
splice seam, or synthetic point. Its measured endpoints remain valid. The focused
splice test checks this after output reload. These fixes add no transform state.

No UI snapshot sweep, resource head build, hardware run, or full CI mirror was run
here. The orchestrator owns the final rendering/resource gates and integrated
adversarial review. Hardware acceptance remains pending; no connected device is
required for the independent integration and simulator work.

## Adversarial review corrections

Correction commit: `2c5e7638`. All four reported blockers are addressed:

- Unique attribution now reports a read error when a competing edge cannot be
  decoded. A two-lane fault source tests node and interior-anchor queries.
- The Swift densifier leaves interior heights unknown when the incoming segment
  is incomplete, while preserving measured endpoints.
- The compact elevation band is unavailable for incomplete segments or failed
  metadata, geometry reads, or decode. It no longer skips an unreadable chunk and
  fills the missing band from neighboring heights.
- Swift widens section-range arithmetic and rejects descriptor overlaps with the
  index and waypoint table. Rust and Swift exercise the same valid envelope and
  two malformed binary vectors.

Focused whole-suite checks after these fixes:

- `cargo test -p obc-route --test nav --test profile` (48 and 12 pass)
- `cargo test -p obc-vectors --test vectors -- --ignored` (shared-vector generator)
- `cargo test -p obc-vectors --test vectors` (19 pass, generator ignored)
- `swift test --package-path companion-ios/Packages/OBCKit --filter RouteObjectCodecTests`
  (12 pass)
- `cargo clippy -p obc-route -p obc-reader -p obc-vectors --all-targets -- -D warnings`
- `./tools/obc suites check` (73 suites, 358 execution units)
- `cargo fmt --all`, `git diff --check`, and `python3 docs/build_docs.py --check-links`

The next review covers only this correction delta. No resource build, rendering
sweep, hardware test, or large map repack was repeated. Final maps will use the
RA09 OBCM v16 header extension and preserve these v15 graph/place semantics.

## Review focus and remaining work

- Independently review incoming segment ownership at chunk and splice boundaries,
  optional-section bounds, and conservative GPX attribution.
- Implement RA04/RA05 descriptor-aware transforms and source binding through the
  reserved contract, then compare preview and committed facts from the same bytes.
- Run the final pinned v16 real-map simulator scenarios and resource/layout gates.

## Current mainline and browser upload

The branch includes the mainline weather removal. The merge preserves the local
UTC-offset authority field, the complete route envelopes, and the current map
version vectors. The shared vector suite passes after the merge. Exact App
allocation records still require the next CI measurement for this combined head.

Commit `202c372d` updates the browser header reader used before route upload and
rename. It now requires the OBCR v4 header (160 bytes). The vector tests refuse an
old version and a truncated extension. The route-name helper uses the same current
header. All 65 builder test files pass: 980 tests. The three normal WASM bridges
were built before the test run. An initial run had missing generated bridges and
an old test-helper header; those setup and fixture errors were resolved.

`obc suites check` passes. No new public behavior documentation is required: the
existing OBCR v4 contract describes this change. No snapshot sweep, resource image,
or hardware run was added. The merge and browser correction need a delta review.
