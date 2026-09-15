# RA07 — Build What's next from real route windows and the full timeline

Parent: #1734. Depends on RA02, RA03, RA05 and RA06. Keep the reviewed overview and Explore ahead.
Use the production Up Ahead merge/filter behavior as the base, not the mock's small entry list.
Route-window work can start after RA02/RA03/RA05; final details integration uses RA06.

Implementation dependencies: [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740), [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741).

## Shared route interval

Read the accepted journey from Navigator, including visit approach, stop, return and remaining tail.
Define the overview as [anchor, min(anchor + selected range, journey end)] with 5 km and 10 km
presets. The profile, interval ascent/descent, climb selection and service queries use that same
axis. A leg end is not the destination. Freeze the browsing anchor until explicit refresh; route
identity/revision changes invalidate it. Keep current-position/pass cues separate.

Reuse `obc-route/src/{profile,climb,climb_profile}.rs`, the existing climb detector and grade colors
from `screen/climb.rs`. RA03 supplies validity and interval metrics. Show genuine flat terrain as
flat; show missing elevation as unknown/gaps. Do not calculate grade from envelope min/max or fill a
gap with easy terrain. Use a sensible low-relief scale and bounded samples.

Show the active climb when already on one, otherwise the next climb whose start is within the
window. A climb crossing the end can show its full known stats, labeled as climb stats rather than
interval totals. Show the next custom waypoint from either generic or categorized source, using its
authored name and explicit distance even if beyond the profile window. Do not infer lunch/rest, add
narrative relationship sentences, or repeat Now/range endpoint labels.

Water and shop summaries use RA02's opening-aware query. A completed empty interval differs from
partial/failed coverage. No last-water, next-after-this-stop, service-gap warning, all-route climb
list or surface bar is added in this epic.

## Explore ahead and details

Extend `screen/up_ahead.rs`, `corridor.rs`, App preparation and existing `UpAheadScope`. Preserve
route order, first-unpassed selection, signed lateral offset, passed rows, frozen membership,
category reset on fresh entry, stored source preference, and Back restoring selection/scroll.
Stream/page all available entries in the selected window; 16 loaded POIs is not the whole list.
Merge by route occurrence and stable identity so ties, loops and rejoin spans do not duplicate or
lose entries. Preserve generic waypoint filter semantics from current tests.

Keep Everything/Water/Campsite/Lodging/Resupply/Pharmacy/Bike shop plus the new train category and
Both/Waypoints only/Map POIs only. Climbs appear in Everything with Both; category/source choices
otherwise retain their established meanings. State this in tests, not implicit mock behavior.
Filters affect the timeline, not the overview's terrain/custom-waypoint facts. Down + Back opens the
existing filter drawer. Use RA02's current-hours rechecks and frozen identity/cursor contract; a
closing unselected service disappears without reranking the surviving route occurrences.

Custom waypoint selection opens authored waypoint details, never Add stop for the same waypoint.
Discovered POIs use RA06 details and RA05 visit review. Share these paths and existing request
scheduling; the overview must not starve per-category next-POI consumers or add a second cache
owner.

## Acceptance and verification

Test active/cross-window climbs, ascent/descent, true flat versus missing elevation, end-of-route,
next custom beyond range, categorized and generic waypoints, >16 entries, equal-distance ties, loop
occurrences and all source/category combinations. Replay a visit across both phases, replace its
route, rewind and change the range while work is pending. Test no route/coverage, filtered empty and
I/O failure. Use route/App/reader whole suites, real Swiss/Monaco fixture suites and named
overview/timeline/details captures. Remove mock profile, fictional summaries and fixed entry arrays
from the normal runtime.
