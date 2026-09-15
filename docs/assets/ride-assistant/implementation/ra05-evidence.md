# Visit implementation evidence

This change supplies the shared route builder, Navigator requests and phases, and host executor
for issue #1740. The separately implemented board adapter is included in this branch. Find Place and Landmark screens
call the shared request and preview methods; this change does not redraw their approved layouts.

## Behavior and bounds

- A source-bound request requires an explicit mapped approach and a compatible profile. A direct
  destination checks its final stored graph point against the approach. Visit composition checks
  the outbound endpoint and each leg/tail seam. It does not draw a rideable line to the display point.
- The final output contains outbound, return/rejoin, original tail, all remaining stored waypoints,
  and the visit descriptor. The writer uses one final output and one reusable leg. It streams the
  complete waypoint section, with original source and ordinal provenance; the 32-entry display
  cache does not limit the result.
- The planner compares complete actual routes. It tries out-and-back, one constrained forward
  rejoin, and at most one reconstruction of out-and-back: at most six graph searches. Cancellation
  after departure constructs one real connector to the accepted route's preserved original tail.
- `VisitBuilder::init_in_place` and `init_return_in_place` initialize the emitter field by field.
  The board adapter places it beside the planner in the existing navigation arena and asserts the
  simultaneous layout and alignment. No resident
  waypoint capacity, open-object limit, reservation limit, or resource ceiling changed here.
- Arrival and rejoin change checkpoint phases on the accepted bytes. Matcher ceilings prevent a
  repeated coordinate on a later phase from becoming current progress. The newest raw fix is
  replayed after a phase ACK. Recovery suppresses arrival information. Rejoin drops the original
  dependency. Back/Dismiss does not change geometry or Recorder.
- Before departure, cancellation clears the checkpoint and restores the exact original route and
  departure progress after ACK. After departure, the connector needs normal immutable review and
  acceptance. Another visit, Easier route, and incompatible detours remain unavailable while a
  visit is active.
- Find Place can freeze one bound review context, inspect exact arrival/complete elevation facts,
  cancel the candidate, wait for physical release, and plan the next candidate from the same origin.
  Candidate shape staging reuses the existing preview buffer and requires the exact commit token.

## Validation

The following whole suites pass on the reconciled parent with weather removed:

- `./tools/obc test -p obc-route -p obc-app -p obc-host-core -p obc-storage`: all selected suites pass.
  This includes App library 940 tests, storage library 195 tests, route contract/planner/transform
  binaries, host executor/storage binaries, and integration suites.
- `cargo test -p obc-app --lib`: final phase repaint and arrival tests pass, 940 tests.
- `cargo test -p obc-host-core --lib`: final immutable Visit acceptance test passes, 62 tests.
- `cargo clippy -p obc-route -p obc-app -p obc-host-core --all-targets -- -D warnings`: passes.
- The board adapter author ran `cargo check --release` in the board root, the complete production
  board executor suite (11 tests), scoped Clippy, and registry checks. All pass. Its storage traces
  cover allocation, write, seal, publication, cancellation, source change, and unknown durability.
  The combined merge was clean and did not change the reviewed adapter APIs.
- `./tools/obc suites check`: passes, 69 suites and 323 execution units.
- `python3 docs/build_docs.py --check-links`: passes. Workspace formatting and diff checks pass.

The shared Visit suite covers 48 repeated-name/off-route annotations, exact provenance and totals,
clipped forward access, the six-search bound, missing/profile-incompatible access, disconnected
seams, a real cancellation connector, measured arrival ascent, corrupt header facts, and direct
approach refusal. App tests cover stationary acceptance, a parallel pass, pending phase writes,
latest-fix replay, ignored arrival state, and recovery suppression. Host tests run actual packed
map searches, complete-variant comparison, a one-search connector, immutable publication/review,
Metadata acceptance, source protection, and reboot Resume admission.

The pinned external route command `./tools/obc test fixtures -p obc-route` does not pass yet.
The `nav` binary has 49 passes and three existing Grimsel expectation failures: measured ascent is
0 m; GPX re-import reports +121 m against the planned +0 m; and profile 0's route hash is
17410155803779901841 rather than 15485656470267133881. The integration owner owns the final regional
map/DEM pack and expectation reconciliation. No expected hash or elevation bound was weakened here.
An attempted fixture dry run synced the existing test profile, then Cargo rejected `--dry-run`;
the actual fixture command above was subsequently run. It did not start a regional bake.

## Remaining integrated acceptance

- Independently review the shared and board implementation, including its simultaneous arena layout
  and source/failure traces. Read the target layout from existing check artifacts; do not infer
  resource headroom from removed weather code.
- Resolve the pinned Grimsel failures on the final map format. Run production simulator replay
  with real offline inputs through out-and-back, forward rejoin, repeated coordinates, parallel
  road, dwell, cancellation before/after departure, and route end.
- Assert one Recorder session and a continuous sample sequence across those integrated traces.
  The shared phase tests and host planner tests do not substitute for that replay.
- Complete independent adversarial review, fix findings, and run delta review. CI and the existing
  integrated resource gates must pass before merge. No resource image or base image was built here.
- The device is not connected. Hardware acceptance remains pending; it does not block independent
  simulator, source, lifecycle, and UI work.

## Hardware checklist and artifacts to prepare

Use the final fixture manifest and immutable source hashes from the integration release. Include
the accepted candidate OBCR, its original route, map and DEM inputs, the GPS replay, expected phase
anchors and costs, the build revision, and the simulator trace in the device test bundle.

1. Copy the exact bundle to the card and verify all payload hashes before the run.
2. Accept one Visit and record its Metadata checkpoint and candidate fingerprint.
3. Ride outbound, dwell, dismiss or ignore arrival, and return. Confirm no route publication or
   Recorder restart at arrival; confirm original protection ends only after durable rejoin.
4. Cancel at departure, then repeat with cancellation away from the original. Accept only the
   measured connector preview in the second case.
5. Repeat at a loop crossing and on the parallel-road trace. Check progress occurrence and phase.
6. Interrupt power during candidate seal/publication and phase checkpoint write. Remount and
   confirm either complete old or complete new state, with no unaccepted route activation.
7. Test source replacement/deletion refusal, card change, failed reads, and Resume after reboot.
8. Record stack/arena high-water marks and compare the shipping image with the unchanged resource
   baseline and ceilings. Retain logs and mark hardware acceptance only after these checks run.

## Explicit cleanup integration

Integration `f04b4ea2` takes the reviewed assistant-only Metadata lifecycle.
Commits `73b7e9fa`, `a81a946b`, and `f82bd517` update the added Visit code to
its current names and capabilities: assistant codec, Metadata outcome/token,
current board replay support, and the board catalog scope helper. The shared
App and host package suites pass; `cargo check --release --locked` from
`firmware/obc-fw-nrf54l` passes after the final helper correction.

Parent `a92c45c8` also includes the independently reviewed accepted-route bit
consumers and corridor encounter fix. No route expiry or auto-retention policy
was restored. No public documentation changed from this composition. Independent
integration delta review and green CI remain required. No shipping image, UI
sweep, or physical-device test was run.

## Current CI composition

CI run `34944597121`, board job `104301093080`, measured App 49,080 bytes
and navigation arena arm 97,440 bytes. The exact records now match the image.
Linked resident is 304,832 bytes, flash 1,587,584 bytes, and residual stack
54,592 bytes. The 131,072-byte arena, 132,096-byte uninitialized section,
9,784-byte poll frame, 4,048-byte task body, and 7,768-byte boot chain are unchanged.
Margin over recorded hardware deep-ride high-water is 17,576 bytes; the floor
stays 8,704 bytes. No device limit or hardware measurement changed.

The CI Clippy job reported redundant nested borrows in three Visit arena
accesses. Each now binds the correct phase's arm once before field access.
This does not change the arena layout or ownership. Parent snapshot format and
detail preparation fixes are included. Final CI and independent delta review
remain required. No local image, sweep, or base rebuild ran.

## Quantized return seam

Code commit `c54f2e5b` coalesces leg and tail endpoints within the existing 1 m mapped-approach
tolerance. It retains the preceding endpoint and omits the duplicate point. It adds no connector.
A coordinate change marks the boundary elevation as incomplete. Larger gaps still reject the
candidate. Source identity, route occurrence, original anchors, and access checks do not change.

The real Grimsel web journey exposed this boundary: the stored return endpoint is
`(8337028, 46576671)` and the imported GPX tail begins at `(8337021, 46576670)`.
The gap is about 0.55 m. The new contract uses these coordinates, checks the exact emitted point
sequence and original anchors, and rejects a tail outside the 1 m tolerance.

Validation on this code:

- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra06-find-place/target ./tools/obc test -p obc-route`: all selected whole suites pass.
- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra06-find-place/target cargo clippy -p obc-route --all-targets -- -D warnings`: passes.
- `./tools/obc suites check`: passes.
- `cargo fmt --all` and formatting in each standalone Cargo root: pass.
- `python3 docs/build_docs.py --check-links`: passes.

Exact logs are `/Users/timo/Documents/OSM-agents/ra05-seam-{tests,clippy,registry,docs}.log`.
The diagnostic real-map run is `/Users/timo/Documents/OSM-agents/ra06-tour-seam-diagnosis.log`;
it records the rejected seam before this fix. The separate tour integration records the final
production acceptance result. No UI sweep, resource image, full CI mirror, or device test was run.
No public conceptual documentation changed.

## Snapshot route imports

The UI sweep now stages only its two intended route vectors, `route-plain.obcr` and
`route-waypoints.obcr`. The vector directory also contains invalid Visit overlap inputs for
format contracts; it is not an importable simulator route directory. Those negative inputs
remain in the format suite. The staged RouteMenu named frame matches the existing manifest hash
`4fcf777f6c806d274823f9fedc659e8efa8beb794907dc8c439d008cbf0ebfbe` exactly.
Shell syntax, suite registry and diff checks pass. No local sweep or image build was repeated.
