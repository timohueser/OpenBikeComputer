# RA02 — Make place queries opening-aware, complete and reusable

Parent: #1734. Depends on RA01. This is the common eligibility/query path for Find a place, What's
next and linked landmarks; do not implement separate hour filters in their screens.

Implementation dependencies: [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736).

## Existing owners

Extend `obc-formats/src/obcm.rs`, `host/obc-pack/src/{poi,hours,ingest}.rs`,
`host/obcm-assemble/src/poi.rs`, `obc-reader/src/{hours,reader/poi}.rs`, and the App-owned
`PoiScratch`/`CorridorScratch` preparation path. Keep draw functions free of I/O. Reuse the quadtree
and bounded read windows. Current nearest/corridor buffers hold 16 and cannot prove complete results
after post-filtering. Current corridor errors must not become queried-empty.

## Fixed behavior

- Preserve service identity across paging, assembly and refresh. Do not merge distinct colocated
  entities solely by latitude/longitude/subtype. Keep source OSM kind+ID or an equivalent stable
  map identity; define composite identity for route occurrences on loops.
- Activate Water, Campsite, Accommodation, Resupply/Shop, Pharmacy, Bike repair, and Train station.
  Add train station classification, shared subtype/category/filter/icon/i18n support end to end.
  Preserve existing campsite access. Keep reserved summit category 7/subtype 19; allocate Train
  station category 8/subtype 20. Derive category iteration and masks from actual service IDs,
  not a contiguous 1..count assumption. Everything includes all seven services, not summits.
  No train timetable or separate transport service.
- Preserve explicit Wiki/entrance/access links needed by landmark/visit producers as build inputs.
  The packer resolves explicit source topology to a bounded runtime approach record: exact mapped
  entrance/access coordinate, source identity, and allowed map-profile mask, or Unavailable. Use
  the source's own graph node or an explicitly linked entrance/access path; proximity alone is
  insufficient. Choose deterministically if several are equivalent. Preserve/remap associations
  during cutting and assembly and bind them to the installed map revision. Service records carry
  this small result; RA09 carries the same approach contract for landmarks. RA05 reads it and
  checks current profile/connectivity instead of rediscovering source joins on-device. Do not
  store article bodies in service POIs.
- Return shared Open/Closed/Unknown. Correct overnight start-day/previous-day spillover, including
  Sunday to Monday, and opening-inclusive/closing-exclusive boundaries. Seasonal, truncated,
  unsupported, failed and missing schedules are Unknown. A holiday exception must not be dropped
  and then used as evidence of Closed. Remove rounding ambiguity: when quarter-hour compilation
  changes a source boundary, mark that schedule uncertain instead of pretending it is exact.
- Evaluate with trusted app UTC plus the configured `utc_offset_min`. The configured local offset
  is authoritative user/device configuration; GPS UTC does not establish it. Reuse clock/offset
  update notifications and invalidate time-derived eligibility at relevant minute/day boundaries.
  Unknown time/offset authority is Unknown. No new global timezone or automatic DST subsystem.
- Exclude only definite Closed, before any result cap, dominance or recommendation ranking. Keep
  Unknown visible, never label it Open. Revalidate selected-place eligibility before acceptance.
  A place closing during review becomes non-actionable without jumping to another selected place.

## Query contract

Provide bounded incremental/continuable nearby and corridor queries, a caller-specified distance
window/radius, deterministic order/ties, and a completion status that distinguishes completed empty,
partial/budget-limited, pending and failure/unavailable. Continuation must not skip or duplicate
items at ties/cell seams. Full timeline paging can use bounded buffers; it cannot truncate at 16.
Freeze membership/order while browsing; explicit refresh restarts from a new source/time anchor.
Map/source/filter change cancels old work. Do not poll or scan the whole map in every draw pass.

Freeze geometric anchors, ordering and the initial eligibility snapshot. Continuation keys bind
source/query generation and stable identity/order, never an array index that shifts when a row is
hidden. Initial-snapshot eligibility still applies to later pages; newly opening candidates enter on
explicit refresh. Recheck bounded visible pages and the selected item at relevant minute/day changes
and before display/action. Suppress newly known-closed unselected entries without reranking
survivors. A selected card/detail becomes an inert Closed state until Back or refresh; never select
its neighbor automatically. Clock-authority or offset changes cancel the eligibility generation;
retain selected identity for restoration after explicit refresh and re-evaluate its current status.
Do not mix pages from different generations. Geometry/identity stability does not freeze an Open
claim. Share this behavior across service and landmark views.

Complete means the stated query has completed inside known loaded coverage. Report separately when
the radius/window extends outside that coverage; a cut edge cannot prove completed empty.

Specify any binary changes in OBCM_Spec, update serializers/assembler/vectors and repack fixtures.
No compatibility/migration layer is required. Old Nearby/Up Ahead consumers should use the improved
query contract rather than preserving a second unsafe path.

## Acceptance and verification

Require at least one real service with a valid encoded approach and successful actual visit, plus an
unrelated nearby entrance that must not qualify. Test >16 closed-near/open-far places,
equal-distance ties, multiple colocated identities, partial map coverage, I/O failure, cancellation
and loop encounters. Test overnight/week rollover, exact boundaries, rounded inputs, seasonal/PH
rules, no schedule, stale boot clock, trusted stamps and changed offsets. Close an unselected item
across a page boundary, change offset mid-query, and page Back/forward through equal-distance ties.
Verify Train station roundtrip and both filter paths, all seven services in Everything, and
unchanged summit content for Peak View. Confirm no silent empty result on reader failure. Use
packer/reader/App whole suites and scoped Clippy; captured Monaco queries belong to the fixture
tier. Update registry and format vectors. RA06/RA07 own final layouts; this issue owns their shared
data behavior.
