# RA04 lifecycle implementation evidence

## Implemented behavior

Navigator owns one frozen review context and one immutable candidate. Host and board publish the
candidate, release construction scratch, and retain the admitted source leases. Preview changes
neither active navigation nor Recorder. Preview figures come from the exact published Route bytes.

Accept checks the origin, route occurrence, profile, map fingerprint, original fingerprint, and
required anchors. Only verified checkpoint publication activates the candidate. A known failure
keeps a valid preview for retry. An uncertain publication fences the operation. Cancel before
submission prevents the write. Cancel after submission resolves the write, clears a recovered
accepted checkpoint, and only then retires the fresh candidate.

A same-card remount resolves the pending edit against the prior or proposed checkpoint after
complete source CRC checks and a sync barrier. New host owners rebind by immutable fingerprints,
not allocation address. Changed maps or original routes cannot authorize an old preview. Board
and host share the exact-original and persisted-avoidance admission predicate.

## Explicit cleanup integration

The owner-approved mainline removed route retention and automatic expiry. Integration commit
`1de46eae` adapts RA04 to that architecture. Commit `667234ac` includes the corrected real-data
parent without repeating its fixture bake or external navigation suite.

The small Metadata handshake owns only a current operation token and uncertainty state. It uses
the existing serialized singleton writer. All 128 ride archive proof rows remain available.
The optional Navigator checkpoint is 96 bytes; the complete transient image is at most 5,248 bytes.
There are no route proof rows, expiry policy, retention UI, or permanent Metadata payload copy.

Acceptance amends the exact current Route catalog entry with `ASSISTANT_ACCEPTED` in the same
transaction that replaces the Metadata image. It adds no mutation kind and does not rewrite Route
payload bytes. An amendment with a changed payload length or CRC cannot carry acceptance. Fresh
replacement bytes cannot inherit the flag. The writer verifies both committed Metadata bytes and
the accepted catalog entry. Clearing a checkpoint preserves accepted route eligibility.

An immutable OBCR candidate flag plus catalog acceptance controls menu and activation eligibility.
Host and board feed one bounded mask to Navigator. Catalog reorder remaps it by exact route ID.
An orphan candidate stays labelled as an unaccepted preview. Explicit cleanup skips the active
route and both checkpoint sources. Explicit replacement or removal of a required source fails.
Invalid Metadata refuses cleanup but does not block unrelated Ride payload mutations.

The board catalog and checkpoint calls share one physical reply slot. Checkpoint submission waits
until the pending catalog ticket is consumed. All Metadata operations still pass through the same
mounted-card writer. No planner arena remains held across checkpoint publication.

Exact contracts are in [Ride archive metadata](../../../../specs/Ride_Archive_Metadata.md),
[flat store](../../../../specs/FLAT_Store_Format.md), and [OBCR](../../../../specs/OBCR_Spec.md).

## Resource ownership

The original route stays readable during planning and review. Terminal cleanup releases its hold.
At checkpoint publication, planner scratch and the construction reservation are already released.
The same writer uses temporary Metadata allocation and bounded readback scratch. Two reservations,
five open handles, and the existing navigation arena remain the limits.

A const-only ARM layout read from the final board check's existing rmeta gives:

| Type | Bytes |
| --- | ---: |
| App | 48,928 |
| NavigatorMachine | 9,568 |
| MetadataEffect | 40 |
| MetadataOutcome | 8 |

This reads type layout only. It is not a linked firmware resource measurement. The orchestrator
owns the single final integrated resource build and all exact resource gates. No capacity or
resource ceiling was raised, and no base image was rebuilt.

## Validation

- `./tools/obc test -p obc-app -p obc-storage -p obc-host-core -p obc-link`: all App, host and link
  suites pass. App library: 877 tests. Host library: 56 tests. Board executor facade: 7 tests.
  Conformance: 27 tests. The first storage run found the old three-bit flag assertion.
- `./tools/obc test -p obc-storage`: after updating that assertion and adding the unrelated-Ride
  mutation case, all 194 tests pass.
- `cargo clippy -p obc-app -p obc-storage -p obc-host-core -p obc-link --all-targets -- -D warnings`:
  passes. The final storage delta also passes scoped Clippy.
- Board `cargo check --release`: passes with the existing target-feature warnings.
- `cargo fmt --all --check` and `cargo fmt --check` in board, boot, and desktop roots: pass.
- `./tools/obc suites check`: 68 suites and 320 execution units pass registry validation.
- `python3 docs/build_docs.py --check-links`: all internal links pass.

The storage suite cuts every media operation before and after checkpoint acceptance and clear.
It recovers only a complete prior or accepted image, preserves archive proofs, and checks that
catalog acceptance publishes atomically with the checkpoint. It covers source replacement,
Metadata CAS conflicts, full 128-row capacity, exact source validation, and uncertainty fences.

Actual HostLoop tests run the planner and flat-store executor over a hermetic packed map. They
check exact preview figures, physical release before acceptance, orphan visibility, ordinary
selection after clear, explicit Resume, and a fresh-owner remount with unchanged or changed
sources. The Metadata handshake rejects late replies and parks after an uncertain write.

No UI snapshot sweep, linked resource build, full CI mirror, new external-data bake, or physical
hardware test was run for this adaptation. The orchestrator's corrected fixture passed its own
52-test external navigation suite; that evidence remains in the RA03 validation note.

## Remaining acceptance

Independent adversarial review and green CI remain required. The orchestrator owns integrated
resource gates, the final UI snapshot sweep, and production simulator replay with the pinned
regional offline package. RA05 owns descriptor-backed visits and phases; RA10 owns approved review
and Resume screens; RA13 owns final acceptance traces.

Hardware acceptance remains pending because the device is not connected. Prepare a final image
and a card with the pinned map, ordinary original route, candidate, and 128 archive proofs:

1. Start Recorder and preview a plan. Confirm the old route and recording continue.
2. Accept and cut power at each publication boundary. Confirm prior state or a complete Resume offer.
3. Cancel before and after submission. Confirm only the fresh unaccepted candidate can be retired.
4. Cut power during clear. Confirm accepted route eligibility and all archive proofs survive.
5. Replace the card and attempt Resume. Confirm old-card work cannot activate.
6. Run explicit cleanup with an active visit. Confirm active, original, and accepted journey sources
   remain readable, while other eligible old routes can be removed.
7. Clear the journey, then run cleanup. Confirm released route dependencies can be removed.

## Catalog flag consumer review fix

Commit `cb06fb05` adds catalog acceptance bit 3 to the Swift decoder's known flags and names the
same bit in the browser vocabulary. Both protocol vector suites exercise a LIST page with an
accepted Route beside a recording Ride. Swift still rejects every undefined bit from 4 through 15.

- `swift test --package-path companion-ios/Packages/OBCKit --filter OBCProtocolV4Tests`: all 19 tests
  in the four protocol suites pass.
- `npm test -- src/lib/usb/vectors.test.ts` in `builder/app`: all 92 vector tests pass.
- `./tools/obc suites check` and documentation links pass.

No Rust suite, linked resource build, image, UI snapshot sweep, or hardware run was repeated for
this client-only delta. Independent delta review and CI remain required before merge.
