# RA04 — Add explicit route review to the existing Navigator lifecycle

Parent: #1734. Depends on RA03. This is the one shared planning/review owner for visits and Easier
route, not another assistant coordinator.

Implementation dependencies: [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738).

## Reuse and change

Extend `NavigatorIntent`, `PlannerWork`, effects/outcomes and operation-token handling in
`obc-app/src/navigator.rs`. Reuse `host/obc-host-core/src/nav.rs`, shared planner dispatch, board
`src/detour.rs`, tagged `NavGuard`, sealed temporary storage and resumable transforms. Ordinary
PlanRoute currently auto-commits/adopts. Assistant requests must instead stop at an immutable
preview and wait for explicit acceptance. Keep normal PlanRoute and existing detour behavior
correct; do not route this around Navigator through a screen or simulator callback.

## Required operation contract

Capture purpose, exact map/store/route identities and revisions, profile, current route occurrence
and progress, origin fix, required route anchors, request generation and measured facts policy. Plan
with the existing bounded acquire/step/commit/release operations. Distinguish workspace refusal,
map/access unavailable, exhausted search, NoPath, I/O/commit failure, cancellation and stale source.
A failure to run is not NoPath. Release acknowledges physical resource return on every terminal.

Keep one planner arena and at most one fully materialized unaccepted route candidate. Alternative
cards may retain small stamped descriptors; switching the materialized candidate may run visible,
cancellable work. Never retain three full routes/arenas in App. Accept binds the exact reviewed
bytes and metrics. If regenerated output differs from a cached descriptor, show the new preview
before it can be accepted. Commit failure retains a valid preview for retry where current storage
semantics permit it. Cancel removes unaccepted fresh publications without deleting original data.

Freeze browsing; do not continuously restart on each GPS update. At acceptance, reject old source,
profile or changed required-anchor stamps. Initial movement tolerance: 50 m along the same route
occurrence and 30 m lateral displacement; outside it, prepare a fresh origin preview. A loop or
ambiguous match cannot pass just because its coordinates are close. Define these named study policy
constants with tests; do not reinterpret stale costs as current facts.

Accepted visits use one ordinary derived OBCR containing outbound, return/rejoin and the original
tail, with a small exact visit descriptor. This preserves ordinary route consumers and avoids
another publication at arrival. RA05 owns construction and phase behavior.

## Durable acceptance and restart

Active navigation is not persisted today. This issue adds recovery for the most recently accepted
Assistant journey only. On boot, offer Resume after exact card/route validation and phase-safe
matching. Do not automatically resume ordinary routes or Recorder. Without an existing Assistant
checkpoint, a crash keeps ordinary boot behavior, not an invented restored navigation session.

Add one versioned optional visit descriptor to OBCR: stop/rejoin/entry cumulative anchors, exact
original StoreId/ObjectId/Revision and original progress anchors, plus target identity/coordinate.
RA03 owns the route-facts contract; coordinate this descriptor and waypoint provenance with the same
spec and all route producers/consumers.

Add one optional versioned Navigator checkpoint section to the existing card-local Metadata
singleton. It contains the full accepted-route fingerprint (StoreId/ObjectId/Revision/length/CRC),
visit phase, and matching/progress anchor. This is new data in the existing object, not an existing
checkpoint or a new database. Navigator owns only checkpoint policy. RetentionMachine keeps
exclusive retention/archive policy ownership. Use one serialized read/modify/publish path for all
Metadata mutations, preserving the other section from the current complete image and checking its
CAS. Separate checkpoint-target validation from retention-row validation. Preserve archive proof,
complete catalog reconciliation, exact source validation, CRC, readback and remount fences. Extend
`specs/Retention_Metadata.md` and its vectors. Account for the added encoded bytes in the current
7,712-byte maximum/workspace placement; no new permanent App copy or lost proof rows.

Publish the complete derived Route, release its construction reservation, then commit the Metadata
checkpoint with the freed writer slot. Activate only after verified acknowledgement. The checkpoint
publication is the durable acceptance point. Use these three outcomes:

- Definitely unpublished: old checkpoint is authoritative; keep a valid preview for retry.
- Verified committed: new checkpoint is authoritative; activate those exact route bytes.
- Uncertain/fenced: acceptance is unresolved. Do not report success, delete the possibly accepted
  route, or claim the old phase won. Keep retained bytes readable and Recorder independent. Park
  fresh mutations until remount, sync barrier and recovered-head inspection resolve the result.

Before physical checkpoint submission, an admitted cancel prevents submission. After submission,
serialize cancellation behind its resolved durable outcome; it cannot revoke an in-flight write.
After a committed result, cancel is a new accepted-journey change. Manual normal-route activation or
navigation stop similarly clears/supersedes the Assistant checkpoint before reporting durable
success; it does not add general persistence for normal navigation.

Apply the same outcome rules to phase writes. While pending, keep the accepted route readable and
retain the latest trustworthy matching/progress anchor. Reconcile it after acknowledgement without
jumping across an ambiguous crossing; recording and already accepted geometry remain independent.
Use replay-safe phase transitions. Arrival is once per live visit, but informational card delivery
is not exactly-once across crashes: suppress the card during recovery. Test stop and rejoin fixes
that arrive while a phase write is pending, as well as every publish/acknowledgement boundary.

## Accepted avoidance compatibility

Existing detour output loses its request-local avoided corridor. Add a small persisted provenance
flag to newly committed detour OBCR and the accepted Navigator context: an avoidance exists that
Assistant cannot reconstruct. Propagate it through trim/splice/reload and request stamps. For v1,
refuse Assistant route changes on this context with a clear unavailable reason; information views
remain usable. This is safer and smaller than introducing a persistent closure list. Ordinary
selection of another route uses that route's own provenance, not a name heuristic. Do not claim
preservation of unknown historical blockages in imported/legacy data. No new Road blocked UI.

## Constrained-resource and recovery criteria

Document phase-by-phase source-hold and reservation ownership. Keep `MAX_RESERVATIONS = 2`,
`MAX_OPEN_OBJECTS = 5`, the current navigation arena and resident/stack limits. Release planner
scratch before the preview is rendered. No giant stack temporary or host Vec policy copied into
firmware. Count map, active/retained original routes, transfer, candidate/swap and temporary
Metadata readback explicitly. Refuse incompatible concurrency; do not use a historical six-hold
count. RA05 specifies the two-reservation assembly. Protect its exact original dependency from
automatic expiry and refuse explicit replacement/removal while needed, rather than losing cancel
recovery.

Publication, descriptor update, power loss and same-card reopen must resolve to an authoritative
prior checkpoint or complete accepted one, never half a visit. Uncertain writes remain fenced until
reconciliation; lack of a prior checkpoint means ordinary boot behavior. Ambiguous post-reboot
progress must not silently jump across stop/rejoin phases. Card replacement invalidates old work and
references. Late replies cannot install obsolete plans or free a newly reused reservation.

## Verification and completion

Use actual host executor traces plus Navigator/App and flat-store suites: preview unchanged active
route/recording, exact accepted bytes, cancellation during search/write/publish/release, retry,
source replacement, storage contention, stale replies and reboot boundaries. Include concurrent
archive-proof and checkpoint updates, refused/uncertain phase commits, explicit Resume, provenance
reload and prevention of Assistant routes through previously avoided roads. Include board adapter
compile/placement checks as changes land; do not defer feasibility to RA13. Resource measurement
uses the one final head build coordinated by RA13, not a base rebuild. This issue closes only when
both host and board implement the same operation contract; UI mocks do not count as parity.
