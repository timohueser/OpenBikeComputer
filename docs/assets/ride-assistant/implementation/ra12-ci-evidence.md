# Production Assistant CI contracts

The normal Assistant replaces the Bluetooth quick-drawer switch. Bluetooth is available in
Settings. The one-home guard now requires six drawer-written settings, and permits no duplicate
Bluetooth editor. Its parser still checks all 15 context row literals and 11 distinct labels.

The first CI image at `26b8f70b` measured App as 52,312 bytes. Run 34953932805, job
104331379554, stopped on the exact allocation check: 51,528 to 52,312 bytes. Only this compile-time
record changes. Resident, scratch, stack, flash limits and hardware high-water records stay as
recorded. Final integrated resource checks remain required.

Twelve new named snapshots were captured with the production simulator built from `4f4cdde7`.
Each used the exact snapshot recipe and the pinned v16 Grimsel or West Cork package. All twelve
frames were visually checked before their hashes were recorded. Fourteen removed recipes no
longer have manifest entries. The complete manifest has 258 entries. Existing changed frames
still require the CI sweep and visual delta check; these captures are not a complete sweep.

The new static Cork and selected-place recipes now supply `--heading 0` with their center. The
simulator center alone selects a camera position; it does not supply a GPS fix. An initial probe
without this flag correctly displayed GPS required and could not open a photo. The repaired
photo shows Dunlough Castle, Sources shows page 1 of 16, and Visit shows the real 110 m connection.
The Find water frame correctly reports no routeable choices. Easier reports no useful route.
The four language captures preserve the reviewed ordinary Assistant question list.

Validation: the complete firmware Python suite (86 tests), one-home guard, suite registry, shell
syntax and diff checks pass. No local snapshot sweep, shipping image, base rebuild or device test
ran for this correction. Public conceptual behavior is unchanged by these test and measurement
corrections. Hardware acceptance remains pending.

The real Monaco Detour commit selects the normal Climb guidance screen after its final location
poll. The former Map expectation stopped CI before the rest of the sweep. The named capture now
expects Climb: the planner reports a 1,333 m connector, +533 m versus the replaced span, and a
successful spliced route publication. The frame shows the accepted route's climb profile. Its
hash was checked visually; the complete firmware Python suite and registry pass again. This is
one named capture, not a repeated snapshot sweep.


## Remaining snapshot recipe corrections

CI run 34956291887 at `4722ad52` passed Rust tests and stopped in the snapshot step before the
manifest check. The route-swap recipe selected the old fourth context row. Routes is now third.
Commit `d872a314` corrects that recipe in English and the three translated captures. The named
route-swap captures reach RouteSwap and match their existing hashes in all four languages.

The CI artifact contains 125 of the 258 expected frames. Visual review found 34 changed frames:

- `climb`, `detour-chooser`, `detour-preview`
- `firmware`, `firmware-recording`
- `map-context`, `map-context-live`, `ride-context`, `menu`, `menu-pois`
- `poi-detail`, `poi-detail-closed`, `poi-detail-split-hours`
- `route-plan-context`, `route-plan-biketype-editor`, `routemenu-trips`
- `routeoverview-est-time`, `routeoverview-est-time-flat`, `statistics-eta`, `statistics-eta-flat`
- `stats-next-category`, `stats-wpt`
- `up-ahead`, `up-ahead-context`, `up-ahead-filter-editor`, `up-ahead-noroute`
- `up-ahead-nothing-waypoints`, `up-ahead-outside-map`, `up-ahead-poi-detail`, `up-ahead-pois-only`
- `up-ahead-sources-editor`, `up-ahead-water`, `up-ahead-waypoints-only`, `up-ahead-waypoints-only-water`

The reviewed frames show the current Assistant entries, v16 map version, route facts and costs,
route-access refusal, and Ahead data. Their hashes are recorded without changing the manifest's
frame set. The Water recipe previously ended during its page load. It now completes eight render
steps after the last navigation input. Its named capture shows the requested water page; that
capture supplies its hash instead of the unfinished CI frame.

The five named captures used the existing simulator from `92af3950`, binary SHA-256
`c1a9874f9aae64d035b88d62235e4f355ab0c85d1d07ff9b2f521cec42eea67b`, with pinned v16 Grimsel and Monaco
fixtures. The route-swap and Ahead screen sources are unchanged between that binary and the CI
head. These captures are not a full sweep. The remaining 133 CI frames have not yet been produced
by this run, so a complete manifest pass remains pending.

Validation: `python3 -m unittest discover -s firmware/tools/tests -v` passed all 86 tests;
`./tools/obc suites check`, `bash -n firmware/ui-snapshots.sh`, and `git diff --check` passed.
No Rust build, local sweep, shipping image, resource rebuild, or hardware run was used. Public
conceptual documentation is unchanged by these snapshot corrections.
