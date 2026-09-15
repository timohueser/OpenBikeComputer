# RA04 lifecycle implementation evidence

## Implemented behavior

Navigator owns one frozen review context and one immutable candidate. The host and board
executors publish the candidate before they release planner scratch. Preview does not change
the active route or Recorder. The figures come from the exact published Route bytes.

Acceptance writes the optional checkpoint through RetentionMachine and the existing Metadata
writer. The edit compares the previous checkpoint, preserves all route rows and archive proofs,
and validates exact source fingerprints. The active route changes only after verified publication.
An unpublished failure permits retry. A possibly durable failure retains the fence. Cancellation
after submission resolves that submission and clears an accepted checkpoint before candidate
retirement. A card change cannot submit a checkpoint bound to the old card.

An immutable OBCR candidate flag and an exact-fingerprint Metadata acceptance bit control route
list eligibility. An orphan candidate stays unavailable after reboot. Clearing the checkpoint
preserves acceptance. A replacement at the same ObjectId cannot inherit it. Recovery offers
explicit Resume after payload CRC checks; it does not activate navigation or restart Recorder.
Resume verifies the current map and requires a trustworthy match in the stored phase interval.
The match provider supplies occurrence, progress and lateral distance; it must not infer a match
from a nearby coordinate alone. RA05 supplies visit transitions and RA10 supplies the review UI.

The phase hook serializes progress updates and retains the latest trustworthy fix for replay after
ACK. The accepted original remains protected from expiry, replacement and removal while referenced.
The accepted avoidance flag remains in the Route and checkpoint and blocks another Assistant plan.
Ordinary PlanRoute keeps its existing behavior, with a durable checkpoint clear before replacement.

## Storage and memory

The five-open-object and two-reservation limits are unchanged. There is one navigation arena.
The board can hold the map, active route and retained original during planning (three handles).
A temporary candidate or Metadata reader uses a fourth handle. A transfer can occupy the fifth;
planner arena admission refuses an incompatible transfer. Host clones of the exact map or route
share their existing lease. Candidate preview keeps its fingerprint, not an open candidate reader.

Candidate construction uses one reservation and releases it on publication. Acceptance has no
candidate reservation. Its Metadata publication uses one reservation and one temporary readback
handle. Recovery verifies candidate and original sequentially, closing each reader before the next.
There is no extra filesystem, catalog, permanent Metadata image or checkpoint service. RA05 must
preserve this census when it adds its two-reservation visit composition.

The maximum transient Metadata image is 7,808 bytes, up from 7,712. All 192 existing row slots
remain available. Checkpoint source fingerprints include ObjectId, Revision, payload length and
CRC. StoreId binds the singleton and the Navigator request. Wire details are in
`specs/Retention_Metadata.md` and `specs/OBCR_Spec.md`.

A const-only ARM layout read from the board check's existing rmeta reports App 57,144 bytes and
NavigatorMachine 11,376 bytes. The orchestrator's existing pre-lifecycle census reports App 56,672
and NavigatorMachine 10,888: lifecycle growth is 472 and 488 bytes respectively. ReviewContext is
128 bytes and ReviewedRoute is 64. RetentionEffect remains 56 and NavigatorIntent remains 48;
they do not copy the full context into unrelated slots. The transient checkpoint edit is 208 bytes.
No base image was rebuilt. No resource limit or high-water mark was raised. The integrated resource
gate must account for these bytes before acceptance.

## Validation

- `./tools/obc test -p obc-app -p obc-storage -p obc-route -p obc-host-core`: App library 1,003 tests,
  22 App integration binaries, and host-core library 65 tests pass. The run stops at six existing
  `board_detour` Plan(NoPath) failures also present in the concurrent integration branch. The
  integration agent is investigating that shared routing fixture failure.
- `./tools/obc test -p obc-storage -p obc-route`: all selected suites pass, including 195 storage
  tests and the complete route codec, planner, transform and contract binaries.
- `cargo clippy -p obc-app -p obc-storage -p obc-route -p obc-host-core --all-targets -- -D warnings`: passes.
- `cargo check --release` in `firmware/obc-fw-nrf54l`: passes. Existing target-feature warnings remain.
- Whole Swift `RouteObjectCodecTests`: 12 pass. The candidate flag is accepted; unknown bits fail.
- Whole i18n unit suite: 8 pass.
- `./tools/obc suites check`: passes (74 suites, 360 execution units).
- `python3 docs/build_docs.py --check-links`: passes.
- Workspace and board formatting pass.

Storage tests cut every media operation before and after checkpoint acceptance and clear. They
preserve archive proof rows, recover either complete checkpoint state, and keep acceptance after
clear. They cover source replacement, exact CAS conflicts and uncertain publication fences.
Navigator tests cover movement, occurrence, cancel before/after submission, retry, card changes
and explicit Resume. RetentionMachine tests prove serialization with ordinary stamps and parking
after uncertainty. The host test runs the actual planner and flat-store executor over a hermetic
packed map, checks exact stored preview figures, accepts through Metadata, checks orphan route
eligibility and verifies reboot offers Resume without activation.

## Pending acceptance and integration

Independent adversarial review and CI remain required. The orchestrator owns the integrated
resource build, final UI snapshot sweep and simulator replay with the final offline data package.
No full CI mirror, snapshot sweep, resource image build or hardware test was run here. The shared
board_detour failure remains explicit; it is not hidden by filtering a test function.

RA05 must connect descriptor-backed visit construction, phase matching and fix replay to these
hooks. RA10 connects the approved review layouts and explicit Resume controls. RA13 supplies final
simulator acceptance traces. This core issue does not claim those later feature slices are complete.

Hardware acceptance is pending because the device is not connected. Prepare the final integrated
image and a card with the pinned map, ordinary original route and a candidate. On hardware:

1. Start Recorder and preview a plan. Confirm the old route and recording continue.
2. Accept, cut power at publication and reboot. Confirm either a complete Resume offer or no offer.
3. Cancel before and after submission. Confirm only the fresh candidate can be retired.
4. Cut power during checkpoint clear. Confirm accepted route eligibility and all archive proofs.
5. Replace the card and attempt Resume. Confirm the old card's work cannot activate.
6. Exercise full source holds and transfer contention. Confirm refusal releases scratch and retries
   do not exceed five handles or two reservations.
7. Measure stack high water and resident limits with the normal board acceptance procedure.
