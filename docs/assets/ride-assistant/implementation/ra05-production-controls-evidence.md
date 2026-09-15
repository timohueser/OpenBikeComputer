# Accepted journey production controls

Implementation: `b388a284`, with recovery fixes `c0306a6f` and `fe2151b0`. The branch includes the RA12 production entry and CI fixes through `2680ba23`.

The ordinary Assistant context opens Current visit. Its detail has Back to map and Cancel visit
rows. Cancel calls the existing Visit owner with the current route reader and map identity.
At the departure anchor, activation waits for the checkpoint clear. Away from departure, the
existing ReturnToRoute planner prepares one immutable connector for explicit acceptance.
Back closes a pending save view without revoking that change.

The shared card scheduler delivers the informational arrival card. It defers for a held input,
passkey, recording recovery, and firmware update. Select and Back dismiss the card without a
route or Recorder request. Rejoin clears an ignored card. The existing phase owner suppresses
arrival during recovery.

A recovered checkpoint offers a Resume card. Back keeps guidance inactive. The Assistant context
can reopen the card. Select queues a read through the existing route reader/index. Both the shared
host executor and board reuse that index and the Navigator matcher. The match scans the full saved phase with the existing chunk buffer. A second scan rejects separated near-equal projections and repeated coordinates. No fix, an unmatched position, or a source-read refusal leaves Resume available for retry.
The existing Resume intent and Metadata operation then check exact route and map sources before
activation. There is no extra route buffer, route owner, or Recorder start.

## Automated evidence

Passed from this worktree:

- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra05-visits/target ./tools/obc test -p obc-app -p obc-host-core -p obc-sim`
- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra05-visits/target cargo clippy -p obc-app -p obc-host-core -p obc-sim --all-targets -- -D warnings`
- `CARGO_TARGET_DIR=/Users/timo/Documents/OSM-agents/ra05-visits/firmware/obc-fw-nrf54l/target cargo clippy --locked -- -D warnings`, from `firmware/obc-fw-nrf54l`
- `./tools/obc suites check`
- `cargo fmt --all` and `cargo fmt` in all three standalone Cargo roots
- A final whole `obc-app` run after adding the action-label width checks and shorter translations

The App contracts cover cancellation before departure and after departure, checkpoint-gated
restoration, Back during save, arrival dismissal, ignored arrival at rejoin, and fresh explicit
Resume. The two existing flat-store executor tests now continue through a fresh App boot and
Select on the normal Resume card, more than 50 m from the acceptance position. They verify exact accepted route activation, an updated durable progress anchor, no recording,
and suppressed arrival. These are component tests with a real flat store and small graph.

The first runs exposed two stale RA12 test recipes. The final RA12 parent `26b8f70b` already fixes
both; this branch merges that parent instead of carrying duplicate repairs. A label-width check
caught the longer German action. Short action verbs fit all four supported languages.

## Integrated acceptance still due

Use the production map/card and normal controls for these named scenarios:

1. Accept a Visit, open Current visit, cancel before moving, and verify the original route returns
   after the checkpoint write. Keep recording throughout.
2. Accept a Visit, ride away from the departure, cancel, inspect the real connector, press Back,
   and verify the Visit remains active. Repeat and explicitly accept the connector.
3. Reach the stop, dismiss the arrival card, and continue. Repeat while ignoring the card until
   rejoin. Compare Recorder session and sample sequence.
4. Restart the same persistent card. Verify no active route and no automatic recording. Press
   Resume with a fresh phase-safe fix. Verify the accepted route, current phase, and no arrival card.
5. Replace a required source or use an off-phase fix. Resume must remain unavailable without
   starting guidance. Exercise checkpoint failure and recovery on the normal card.

The orchestrator owns real offline simulator captures, final integrated review, CI resource
records, and the single final snapshot sweep and shipping build. This change runs neither sweep
nor image build. Physical-device acceptance remains pending; no hardware is connected.

## Adversarial recovery delta

The first independent review found that the frozen-preview movement tolerance was also used for
restart. A checkpoint records phase boundaries, so it can be far behind ordinary progress. The
recovery fix scans the full persisted phase. It stores the uniquely matched progress, occurrence,
and coordinate in the checkpoint before activation. The 50 m acceptance tolerance is unchanged.

The whole `obc-route`, `obc-app`, and `obc-host-core` suites pass after this fix. Added cases recover
more than 500 m into outbound and return phases, refuse an overlapping loop without a unique phase
match, allow a later retry, and find a position beyond the live matcher's 64-segment window. The
existing nonzero-occurrence test now expects the newly matched coordinate in the checkpoint.

Delta validation also includes scoped all-targets Clippy, standalone board Clippy, the suite
registry, formatting, and the documentation link check. The simulator suite was already passed
for the initial control patch; the orchestrator owns final real-data simulator acceptance.

## Integrated recovery corrections

The integrated review found two recovery edges. A restart close to the stop treated the recovered
position as a new departure anchor. Resume now uses the route entry and confirmed progress to retain
proven departure. A new visit still requires movement before arrival.

A refused phase or cancellation write could leave an accepted visit inactive in the review owner.
The owner now retains the authoritative checkpoint and active journey when the write is confirmed
unpublished. Phase reconciliation retries with the latest fix, without waiting for another GPS
sample. A refusal during initial Resume keeps guidance inactive and permits retry. An unknown write
outcome remains fenced until the card resolves it. The screen shows the refusal and keeps the
current visit available.

For `fe2151b0`, the whole `obc-app` suite, App all-targets Clippy, suite registry, workspace formatting,
and diff whitespace check pass. The App contracts cover recovery 5 m and 15 m before the stop,
stationary new acceptance, all three phase transitions, cancellation, initial Resume, definite
write refusal, and uncertain writes resolved to the old checkpoint. They also check retry without a
new GPS sample and dismissal of an arrival card. These tests use the owner's Metadata outcomes.
The flat-store executor contracts passed in the earlier `obc-host-core` validation and were not
rerun for this App-only correction. No simulator rebuild,
snapshot sweep, board image, or hardware run was used for these corrections.
