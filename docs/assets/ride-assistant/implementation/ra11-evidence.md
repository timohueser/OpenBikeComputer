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
