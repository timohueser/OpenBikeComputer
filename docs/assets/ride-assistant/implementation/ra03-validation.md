# RA03 implementation and validation

Implementation commit: `53a9cc65`.
Issue: [RA03 #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738).

## Integration dependency

This branch starts at develop `6abbcd2b`. It must be stacked on RA02 before merge.
RA02 owns the OBCM v15 version bump and rebuilt map fixtures. RA03 uses v15 edge
count bit 15 for complete DEM integration; the graph record length stays unchanged.
Reconcile the small OBCM spec, packer, vector manifest, and synthetic terrain fixture
hunks when stacking. Review the conflict-resolution delta only.

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
package after stacking; do not patch the old content-addressed package in place.

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
- Final changed transform suites: `cargo test -p obc-route --test detour --test transform`
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

No UI snapshot sweep, resource head build, hardware run, or full CI mirror was run
here. The orchestrator owns the final rendering/resource gates and integrated
adversarial review. Hardware acceptance remains pending; no connected device is
required for the independent integration and simulator work.

## Review focus and remaining work

- Independently review incoming segment ownership at chunk and splice boundaries,
  optional-section bounds, and conservative GPX attribution.
- Check transformed waypoint anchors against the final retained geometry; existing
  transform anchor calculations use input cumulative distance and seek from integer
  chunk anchors, so precision across decimation and partial boundaries needs review.
- Integrate RA02 and re-run only the affected conflict-resolution checks.
- Implement RA04/RA05 descriptor-aware transforms and source binding through the
  reserved contract, then compare preview and committed facts from the same bytes.
- Run the final pinned v15 real-map simulator scenarios and resource/layout gates.
