# RA07 validation evidence

The implementation starts at `9d646b9d`, with conceptual documentation in `154ddf6b`.
The reviewed Find parent `f075a732` is merged. Its i18n additions and the What is next additions
are both retained. The parent includes the reviewed continuous-encounter query correction and
the explicit-cleanup MetadataMachine integration.

## Implemented

- `App::open_whats_next()` opens the production overview. The Assistant question can call this
  entry. The legacy Ride drawer remains available until final entry composition.
- One frozen 5 km or 10 km interval feeds route facts, the cached profile, climb selection,
  and the shared corridor query. Route or map replacement requires explicit refresh.
- Ascent and descent use measured route facts. Missing elevations leave gaps and unknown totals.
  Grade colors use the existing measured profile grades. Full climb statistics can cross the end
  of the window. The next authored waypoint can be beyond the window.
- Four display rows merge all stored authored records, climbs, and the existing POI page. Forward
  and reverse continuations use occurrence identities. No second POI or route-profile cache is added.
- The existing filter drawer supports Train and source scope. Map places use the shared detail
  and Visit path. Authored details have no Add stop. Back preserves the selected occurrence.
- Distance and ascent rows use the frozen anchor; the passed cue uses current progress. Opening
  status updates remove closed unselected places without reordering surviving rows.

## Checks

- `./tools/obc test -p obc-app`: all suites passed after parent composition.
- `OBC_AHEAD_FRAME_DIR=/tmp/ra07-frames ./tools/obc test -p obc-app --lib`: 908 tests passed
  on the final code. This includes production frame rendering and Back-selection checks.
- `./tools/obc test -p obc-route --test facts`: 6 tests passed.
- `cargo clippy -p obc-app -p obc-route --lib --tests -- -D warnings`: passed.
- `cargo fmt --all`: completed.
- `./tools/obc suites check`: passed, 68 suites and 320 execution units.
- `python3 docs/build_docs.py --check-links`: passed.

The route-window test converts GPX and tests both complete and missing elevation. The paging test
uses a valid OBCR section with 40 authored records, beyond the resident limit of 32, and a packed
map with 23 places including Train. It checks complete forward traversal, reverse pages, source
filtering, and selected-occurrence preservation. Separate tests exercise real packed opening hours
and failed byte-source reads.

The two named frames use the production App and converted test route. They are authored test
inputs, not external-data acceptance. The map deliberately has incomplete corridor coverage, so
service summaries remain unknown. The name is fitted clear of its distance.

- [Overview frame](ra07-frames/overview.png)
- [Timeline frame](ra07-frames/timeline.png)

## Remaining acceptance

Independent adversarial review, green CI, final Assistant entry composition, production simulator
acceptance with the pinned offline ride scenarios, and the final resource measurement remain with
the integration owner. No simulator binary, resource image, full UI snapshot sweep, or physical
hardware check was run in this implementation worktree. Hardware acceptance remains pending.
