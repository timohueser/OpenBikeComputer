# RA06 — Connect Find a place to real queries and complete visit costs

Parent: #1734. Depends on RA02 and RA05. Keep the reviewed map/card UI and use the shared place
identity, opening state, details and visit plan. Replace the mock shop-only input path.

Implementation dependencies: [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740).

## Acquisition and choice policy

Activate all categories named in the epic. Use a bounded nearby search plus the forward route
corridor, with explicit scope and source/completion stamps. Initial scope is 10 km straight-line
radius and the next 20 km of accepted route within the existing 300 m corridor half-width. These are
acquisition limits, not promises of rideable access or global nearest completeness. The radii, On
the way allowance and work counts below are initial tunable implementation policy, not new
owner-approved design thresholds. Keep them together in one named policy; report scope and partial
state in the broader browser, not extra card prose.

Filter known-closed places before candidate capacity and ranking. Deduplicate by stable identity.
Take at most the first eight eligible nearby entries and first eight eligible forward-corridor
entries in deterministic geometric order, alternate sources, and deduplicate. Do not refill without
a bound after deduplication. Run at most 16 distinct candidate visit plans per explicit shortlist
refresh, sequentially through RA05. One request is active; use Navigator tokens and acknowledge
release before the next request or refresh. RA05 separately bounds searches inside each visit.
Source, candidate or planner caps make the result Partial/Nearest found; query completion alone does
not prove complete routed ranking. More places streams RA02's full browser and plans only the
selected item's review, never every visible row. Check actual request/read/work counts in tests.
Keep up to four useful suggestions with measured costs; never force a detour or pad the list. The
initial On the way allowance is at most 400 m added distance for the complete visit. Among eligible
places, retain multiple on-way stops and a useful nearest reachable alternative when its
arrival/complete-visit costs are not dominated. Keep this fixed policy in one small module. Unknown
metrics cannot dominate measured choices. A closed/unreachable place cannot eliminate an eligible
candidate. Show fewer results when calculation or coverage cannot support more.

“Nearest” is only valid within the completed routed candidate scope. If the planner's bounded search
or acquisition cannot establish it, use Nearby/Nearest found with a scope in details. Do not imply a
global optimum. More places opens the full available category browser with continuation, so the
shortlist is not the only route to a particular place.

## Interaction and data ownership

Freeze membership, order, camera bounds and distance/cost anchor while browsing. Up/Down changes
selection; Back from details restores row/scroll. Share the existing POI details layout and RA05
visit preview with What's next and Landmarks. Show hours details from RA02; unknown is not open.
Refresh essential hours/source/origin eligibility on review/acceptance and invalidate stale plans.
Use RA02's frozen identities/current eligibility contract for all pages. Make refresh explicit; do
not silently switch a selected place when it closes or disappears.

The card shows routed distance/ascent to arrival and the complete visit's added cost. Plain
straight-line or corridor distance may be shown while access is unknown, but must be labeled and
cannot masquerade as the completed cost. Loading, no fix, no map coverage, no graph access, partial
results, genuinely empty and I/O failure remain distinguishable. Without an active route, provide a
direct destination preview rather than fake return costs.

Move I/O and planning out of `draw`; reuse App-owned query scratch and Navigator operations. Replace
`assistant_demo/candidates` and static Stop fixtures in production. Keep synthetic cases only as
explicit tests. Do not fork a new detail screen for each entry source.

## Acceptance and verification

Use Monaco dense paging plus real Swiss/Cork access topology. Include nearer-by-air/farther-by-road,
closed-near/open-far, several on-way shops, dominated detour, one/no useful result, unavailable
metrics and a place closing during review. Verify stable browsing while GPS replay progresses, More
places reachability, same identity across entry paths, and continuous recording on acceptance. Run
focused reader/App/host suites and captured fixture suites. Inspect named map/card/detail/ preview
frames; no full UI sweep per child. RA12 owns production entry and final i18n consolidation.
