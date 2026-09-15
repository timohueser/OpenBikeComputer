# Find place validation

Implementation: `5a1ef09f`. Parent: RA05 `48653a1e`.

## Behavior

The shared App acquires up to eight eligible nearby places within 10 km and eight eligible places
within 300 m of the next 20 km of accepted route. It alternates and deduplicates these sources.
It makes at most 16 distinct Visit requests. Each request waits for the previous physical release
acknowledgement. Up to four measured choices remain. The existing page buffers hold the POIs.
The existing catalog preview buffer holds the selected candidate shape.

More places leaves the shortlist and opens the existing category browser with forward and reverse
paging. It does not plan the rows. Place detail starts the same Visit review from either entry.
The host binds the exact loaded map key. The board binds its flat-store planner map key.
The production Find entry is `App::open_find_place`; RA12 owns the final Assistant menu entry and
translation consolidation.

## Checks completed

- `./tools/obc test -p obc-app -p obc-host-core`
- `cargo clippy -p obc-app -p obc-host-core --all-targets -- -D warnings`
- `./tools/obc suites check`
- `./tools/obc test affected --base 48653a1e --dry-run`
- `cargo fmt --all`; `cargo fmt` in the board, bootloader, and desktop roots.
- `python3 docs/build_docs.py --check-links`
- One ARM type census: `RUSTC_BOOTSTRAP=1 cargo rustc -p obc-app --lib --target thumbv8m.main-none-eabihf -- -Zprint-type-sizes`.

The authored host scenario uses normal map serialization, flat-store map and route sources,
HostLoop passes, Navigator Visit planning, candidate publication, and App rendering. It observes
16 initial acquisitions with release between plans. Selection causes one further plan. It reads
the matching preview geometry, changes the trusted clock so the place closes, refuses acceptance,
cancels, waits for release, and opens a later browser page without further planning. The original
route stays active and no checkpoint is written throughout these probes.

Unit coverage includes useful choice ordering, unknown elevation, zero or one result, missing fix,
and source replacement. Existing legacy planner screen tests moved to the crate-private harness;
all 18 tests remain. The change adds no public test-only UI entry point.

One named Find frame was inspected through the normal host renderer. It preserves the reviewed
header, map, marker, and bottom-card geometry. Its authored map has no decorative map features;
it is not evidence for captured regional-map acceptance.

The ARM census reports App 54,096 bytes, UiRuntime 4,944 bytes, FindState 752 bytes, and Screen
96 bytes. This is a type census, not a shipping resource result. No limit or baseline was changed.
The integrated CI resource gate remains required.

## Remaining acceptance

- Independent adversarial review, then delta review for any fixes.
- CI on the final integrated parent, including its resource and frame-size gates.
- Captured Monaco dense paging and Swiss/Cork access examples in the simulator. The current parent
  reads OBCM v15; published landmark inputs use v16. The orchestrator owns that format integration
  and final regional fixture rebuild. No captured-input acceptance is claimed by this PR.
- Named regional shortlist, detail, and preview frames; continuous recording during acceptance.
- Final Assistant entry and translations from RA12.
- Physical-device acceptance remains pending. Use a prepared offline map and route, find a place,
  inspect its costs and hours, accept, visit and return, then inspect the saved recording. Also
  cancel a review and change the map before acceptance. Do not connect or flash a device for this PR.

No UI snapshot sweep, shipping resource image build, full CI mirror, or hardware run was made.
