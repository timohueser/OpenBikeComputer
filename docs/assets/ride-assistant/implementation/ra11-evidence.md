# Easier routes implementation evidence

This change implements issue #1746 on the shared Visit planner, immutable review lifecycle, and
existing navigation arena. The production entry is `App::open_easier_routes(map)`. The caller supplies
the loaded map's exact `RouteSourceKey`. The executor binds the original source and reads its facts.

## Search and review contract

- Each refresh runs one normal-profile trial and two trials for each objective: seven complete
  constrained candidates at most. Each candidate uses sequential searches through every remaining
  stored annotation access point and the destination. The limit is 32 remaining records and 33 legs.
  More records cause an unavailable result; the resident waypoint display window is not a limit
  that can silently discard constraints. Repeated records retain their names, display coordinates,
  lateral offsets, category, elevation, source, and ordinal provenance.
- Climb trials multiply the profile climb weight by two or four, with minimum weights 10 and 20
  and maximum 255. Surface trials multiply rough-surface weights by two or four with saturation.
  Distance trials halve or clear the climb weight. Highway weights, access prohibitions, forbidden
  surfaces, and the saved profile remain unchanged. These bounded trials do not claim an optimum.
- Costs come from the complete stored candidate. The minimum gains are 50 m climb, 500 m rough
  distance, or 500 m total distance. Added distance is limited to the larger of 2 km and 25 percent;
  added ascent is limited to the larger of 100 m and 25 percent. Required elevation facts must be
  complete. Smoother also requires the same attribution map and no increase in unknown surface.
- The batch retains at most three useful descriptors and removes duplicate geometry or equal-cost
  choices. It releases the previous planner before the next trial. The selected descriptor requires
  one separate bounded reconstruction, with the same frozen context, exact CRC, and measured costs.
  This uses the same candidate storage and arena, not a second resident route index.
- The map keeps one camera over the current remaining route and candidate bounds. Current is
  magenta and proposed is blue. Back from review retains the selection. Only an explicit Use action
  accepts the exact reviewed candidate. Saving follows the existing Metadata acknowledgement;
  Recorder continues. Active visits and unresolved avoidance make this action unavailable.

## Validation

These focused whole suites pass:

- `./tools/obc test -p obc-app`, including 881 library tests and all selected integration suites.
- `./tools/obc test -p obc-route -p obc-host-core`, including all route contract/transform suites,
  59 host library tests, and 13 board executor tests.
- `cargo clippy -p obc-route -p obc-app -p obc-host-core --all-targets -- -D warnings`.
- `./tools/obc suites check`: 68 suites and 321 execution units.
- `python3 docs/build_docs.py --check-links`, workspace and standalone formatting, and diff checks.

The production host test runs seven real graph trials and one selected reconstruction, exposes one
useful shorter candidate, and checks explicit acceptance of its stored bytes. It also checks Back,
physical release, and a fresh comparison after acceptance. Route tests verify exact thresholds,
preserved profile prohibitions, complete annotation metadata, a later loop occurrence, and explicit
refusal above the annotation bound. The board owner test runs all seven objectives through the
normal writer and source protocol and checks cancellation, cleanup, and unknown elevation facts.

An ARM const census uses the release `thumbv8m.main-none-eabihf` App library, without a shipping
image build. App is 49,352 bytes, Screen is 96 bytes, and EasierScreen is 68 bytes. This is 272 bytes
above the parent App record of 49,080 bytes. No resource limit or baseline was changed by RA11.
The parent merge only changes CI staging, recorded measurements, and equivalent board borrows;
its shared App and route code is unchanged from the tested parent.

## Remaining integrated acceptance

- Independent adversarial review and CI resource checks remain required before merge.
- RA12 connects the production entry to its loaded-map menu dispatch. The final integration must
  validate the normal simulator path with the pinned regional map and real offline data, including
  a useful climb/surface alternative and unchanged Recorder output.
- The approved map and review geometry is reused. Final named-frame checks and the one permitted
  snapshot sweep belong to integrated acceptance. No local shipping image or snapshot sweep ran.
- Physical-device acceptance remains pending. It must check action latency, cancellation and Back,
  candidate selection, saving, power recovery, and Recorder continuity. No device is connected;
  this does not block independent software work.

## Adversarial review delta

The review identified two production defects. Commit `a77ec8a0` makes the retained review show
unavailable or unknown-save status when Navigator cannot accept it. Use is available only for a
current Preview; Saving remains visible. The board executor also accepts a consumed terminal
anchor, releases its unused leg allocation, and finishes without charging another search.

`./tools/obc test -p obc-app -p obc-host-core` passes after the fixes, including 883 App library
and 14 board executor tests. The added cases cover movement refusal while review is open,
uncertain metadata acknowledgement, retained screen costs without a usable action, and a terminal
loop with a preserved required annotation. Scoped App/host all-target Clippy and the suite registry
pass. No snapshot sweep or shipping image build was repeated. Independent delta review remains
required before merge.

Delta review found that automatic invalidation must not request cancellation while the save
result is unknown. Commit `160c5291` keeps the pending acceptance fenced during Saving and
Unresolved; the disabled review reports the unknown status. Only explicit Back requests cancel.
The App test now recovers both possible exact heads: the committed head returns activation without
an extra clear, and the old head restores the preview without a clear. The whole App suite, scoped
App Clippy, and registry check pass after this correction. Board code is unchanged from `a77ec8a0`.


## Normal entry and named frames

The Ride context now has an Easier action on Statistics, Climb and Ride control. The Map context
retains its five rows. The action waits for the host or board map owner to supply the exact current
source key. It does not add resident source storage. Pending or refused entry cannot cancel an
unrelated review, active Visit or uncertain save. Cancellation requires the admitted Easier context.
RA12 replaces this entry with its approved question page during composition.

The named `easier-routes.png` recipe uses the existing Grimsel map, its stored climb route and its
GPX position at 30 seconds. It starts the route through ordinary controls, polls that GPX position
through the normal location input with `T`, opens the Ride context, and selects Easier. The source
bind and comparison complete. The result is **No easier route**, over the fitted remaining route.
This is not evidence that a useful alternative exists on this input. Useful real climb and surface
alternatives remain part of integrated acceptance.

Only `easier-routes.png` and the changed `ride-context.png` were captured and visually inspected.
Their hashes are in `firmware/ui-snapshots.sha256`. Both use the normal cached map and the existing
`ETAROUTE` fixture directory. The latter now uses the real Grimsel route instead of importing every
format vector: that directory contains intentionally invalid route records. The other existing
snapshot recipes still need the central valid-route fixture staging correction before the final
sweep. No format-rejection vector was removed.

Validation on the entry delta:

- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra05-visits/target ./tools/obc test -p obc-app -p obc-host-core -p obc-sim`
  passed all selected whole suites: App 885 library tests, host 59 library tests, simulator 62 tests,
  and their integration suites. Existing captured-only tests stayed ignored.
- The same packages passed `cargo clippy --all-targets -- -D warnings`.
- `python3 -m unittest discover -s firmware/tools/tests -v` passed all 86 tests, including screen
  coverage, recipe-to-manifest consistency and duplicate-frame policy.
- `./tools/obc suites check` passed: 68 suites and 321 execution units.
- `cargo fmt --all`, standalone board formatting and `git diff --check` passed.
- No full snapshot sweep, shipping image, local resource build or physical-device test ran.
  Independent delta review and CI remain the merge gates.

Retained logs are `/tmp/ra11-entry-final-tests.log`, `/tmp/ra11-entry-final-clippy.log`,
`/tmp/ra11-entry-tools-tests.log`, `/tmp/ra11-entry-registry.log`, `/tmp/ra11-entry-frame.log`
and `/tmp/ra11-entry-drawer.log`. The two inspected PNGs are in the implementation worktree at
`.artifacts/ra11-entry/frames/`. The map input is the immutable `sim-grimsel` package
`b427bce15e08993ebb07d12b5284fb9ad6c20dfb26cfa7aebaa94399b37b87f5` from this branch's catalog.
The track and route are the package's pinned authored Grimsel inputs; the map uses captured OSM
and real terrain. The replay motion is authored, not a recorded field ride.
