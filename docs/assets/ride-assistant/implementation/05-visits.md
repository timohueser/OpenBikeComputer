# RA05 — Build real visits and continue on the accepted journey

Parent: #1734. Depends on RA02 and RA04. This supplies one visit flow to service and landmark
screens, plus the accepted-journey context used by What's next and Easier route.

Implementation dependencies: [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA04 — #1739](https://github.com/timohueser/OpenBikeComputer/issues/1739).

## Route construction

Use the existing planner, resumable trim/splice and fresh route publication. Construct one ordinary
OBCR with current-position approach, the selected stop, a return/rejoin path and the original tail.
Keep explicit phase anchors on its progress axis and original route occurrence. Arrival changes
phase/guidance along these accepted bytes; it does not start a search or publish another route.

Use two reservations with a fixed assembly sequence: final output A remains unsealed; plan each leg
into reusable B, seal B, stream it into A, release B, then reuse B for the next leg. Append the
original tail and emit the waypoint section/header into A. Accumulate RA03 facts from that final
stream, including seams. Keep the final emitter in a named phase-owned partition of the existing
navigation workspace while the planner uses its own scratch/emitter. Prove simultaneous layout and
alignment with assertions before adding UI callers; never place a second large emitter on the stack.
No temporary catalog routes or third reservation. Preview retains sealed A plus a bounded
descriptor; publishing A frees its slot before RA04's Metadata write.

A place has a display coordinate and separately established accessible approach from RA02/RA09's
map-bound coordinate/source/profile-mask record. Validate its profile and graph association. Prefer
explicit mapped access/entrances and profile-compatible graph connectivity. A nearby graph snap
across a river, wall, private boundary or waterfall is not proof of access. If no credible mapped
approach is available, permit information browsing and explain why Visit is unavailable. Never
synthesize a straight rideable final segment to the display coordinate.

Start with a real out-and-back to the departure anchor. Try at most one forward rejoin: the first
usable original-route position at or beyond 1 km forward, clipped to the next preserved waypoint
anchor or destination. Skip it when it cannot preserve all constraints. These are named initial
policy constants. Each visit uses at most six graph-leg searches: two per complete variant plus at
most two to reconstruct the chosen first variant after inspecting the second. Each search keeps the
existing NavPlanner step/node limits; exhaustion remains unavailable/partial. Keep only one
materialized variant, discarding the first before building the second. Prefer a valid forward
variant only if it reduces complete added distance while retaining all anchors; otherwise rebuild
the out-and-back once. Rebuild failure is unavailable, not permission to use stale bytes. Retain
bounded exact-source cost descriptors and show the final actual bytes in review. Compare full visit
and baseline over identical origin/rejoin endpoints, in the correct direction. Use measured
distance/ascent, including both excursion directions and connectors, not lateral offset or graph
weighting as added cost.

## Waypoints and accepted context

Preserve all remaining authored waypoints, text, coordinates, categories and ordering. Identify each
by its exact original route key and stored ordinal; current waypoints have no stable ID. At the
first transform persist this provenance tuple with the waypoint; later transforms carry it
unchanged. Repeated names remain distinct. RA03's route contract owns the field; no external ID
registry or dependency on the historical source bytes is required to read the identity. Retain an
off-route annotation's current projected on-route access position as its route constraint. Use
`RouteReader::position_at(dist_along_m)` for that access anchor. Remap cumulative distance and
lateral offset after the transform; do not turn a raw coordinate or inferred meaning into a new
visit. Explicit user-accepted visit stops remain mandatory. Check the full stored waypoint section,
not the resident 32-entry window. The current splicer drops replaced-span waypoints and caps
retained output; repair the shared transform or refuse unrepresentable output before acceptance. No
silent loss or skip UI in v1.

Keep the original exact source available for cancellation while needed, using existing retention
protection. RA04's descriptor/checkpoint contract covers the complete accepted visit. Protect the
original from automatic expiry and refuse explicit source replacement/deletion while the
live/recoverable visit still needs it; ending/replacing the journey releases that dependency. After
rejoin, commit completed/no-visit checkpoint state, release the original dependency and continue on
the accepted tail. Future Resume uses the standalone derived route and does not require its
historical original. No second confirmation or forced original-route selection is required. Route
identity changes must invalidate derived facts, not Recorder. One active visit at a time; selecting
another does not silently replace it.

## Progress and controls

Use Navigator's route occurrence/progress plus proximity and hysteresis for arrival and rejoin. Use
RA04's pending/uncertain phase-write behavior; suppress informational arrival cards on recovery. Do
not trigger arrival immediately because the acceptance fix is near the stop, or when passing it on a
parallel road. Trigger each phase once. Dwell at the stop is allowed. The arrival card is
informational: Back/Dismiss does not change guidance, and rejoin clears an ignored card.

Before departure, cancel can restore the original route. Off the original route, cancellation needs
an explicit real connector preview; do not teleport progress to a nearby crossing. Back only
dismisses views. Other Assistant questions remain accessible during a visit. Refuse Easier route
during an active visit for v1. Existing detour work that cannot preserve the accepted stop/phase
anchors must also refuse instead of replacing them. A second visit remains unavailable until the
current one ends. RA04's unresolved-avoidance provenance refuses visit routing on a route whose
earlier blocked-road decision cannot be reconstructed. No automatic rerouting, recording restart, or
finish-recording action at POI arrival. Without an active route, Visit is a direct destination route
and promises no unspecified continuation.

## Acceptance and verification

Replay real graph routes through out-and-back, forward rejoin, loop/repeated coordinate, parallel
road, stationary acceptance, dwell, ignored arrival card, cancel before/after departure and route
end. Assert one Recorder session and continuous sample sequence through each. Transform tests cover
more than 32 stored waypoints where the format permits them, off-route annotations, seam ascent,
exact preview/committed totals and original-source deletion/refusal. Exercise actual
executor/storage failure and restart traces. Run whole route/App/host/storage suites and pinned
external fixture suites, not only a mocked transition test. Remove static Stop outbound/continuation
IDs and simulator-injected arrival from the production path.
