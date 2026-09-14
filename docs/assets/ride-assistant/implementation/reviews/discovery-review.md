# Discovery adversarial review — first full round

Reviewed 2026-09-14. Read epic in full, all child headings/dependencies, RA02/RA06/RA07/RA12 in full, and integration contracts in RA01/RA03/RA05/RA09/RA10/RA13. Checked relevant current category/query/clock code. No edits, builds, publication, or test execution.

Status: **not ready until D1–D4 are resolved**. This is a bounded correction pass, not a request for another architecture or broad review.

## Blockers and concrete fixes

### D1 — RA06 Acquisition and choice policy: finite shortlist work is undefined

The 10km-radius plus 20km corridor can contain thousands of eligible POIs. "Sequentially within existing work budgets" limits concurrency or time per step, not total plans. A literal implementation can compute thousands of outbound/return routes before offering four suggestions, despite fitting memory. RA02 Complete on discovery also does not prove completion of this second routed ranking stage.

Fix: give the shortlist a named per-refresh lifetime cap in addition to existing per-step scheduler budgets. A reasonable initial policy is at most 16 distinct candidate visit plans, sourced from at most the first eight eligible nearby entries and first eight eligible forward-corridor entries, with deterministic ordering and deduplication; fewer after dedup is acceptable, rather than unbounded refill. This count is an initial tunable, not a new owner-approved UX contract. RA05 separately bounds work inside one complete visit plan (forward-rejoin candidates, each graph search). The request is Partial/Nearest found when either source/candidate/planner cap prevents completion. More places streams every available eligible result under RA02 and computes a visit only for the selected entry's review. It must not start a graph plan for each item merely because its page is visible. One active plan request, existing Navigator token/cancel/release semantics, explicit new refresh, no new coordinator. Tests assert actual request/read/work counts on a dense fixture, not only shortlist length.

Label 10km/20km/400m and the new counts as initial tunable implementation policy. Only 400m existed in a mock; the user approved layout, not these search thresholds. These parameters belong together so future adjustment does not create multiple contradictory definitions of "on the way".

### D2 — RA02 Fixed behavior + Query contract, RA06/RA07: frozen paging versus current-hours update is ambiguous

The plan both freezes membership to a source/time anchor and invalidates eligibility on every relevant minute/day change. It does not say whether continuation uses its original evaluation time or the new time, whether elapsed time invalidates the whole list/cursor, or how a non-selected known-closed result behaves. Filtering pages at different times can skip/duplicate identity ties, expose a closed item from a previously loaded page, or make a user unable to finish paging a busy list. Selected-item acceptance revalidation alone does not cover the stated current-open display rule.

Fix: define one policy across queries and all screens. Freeze geometry/ranking anchor and stable identities/order; continuation keys bind exact source/query generation and stable ordering, not a shifting array index. Evaluate a bounded page/selected-item opening status at relevant wall-clock changes and before display/action; suppress newly known-closed unselected entries without reranking survivors. A selected detail that becomes closed stays on the same identity with a non-actionable closed state until Back/explicit refresh; it must not switch to a neighbor. Newly opening candidates appear on explicit refresh. If clock/offset changes require a new eligibility snapshot, retain identity for restoration and cancel old continuation cleanly; do not silently mix generations. It is fine to choose another similarly precise small policy, but it must reconcile current-open and stable browsing explicitly. Tests must close an unselected item across a page boundary, change the offset mid-query, then page Back/forward with equal-distance ties.

### D3 — RA07 dependency graph: shared place details producer is absent

RA07 explicitly uses RA06 details, but both the child header and epic graph permit RA07 to complete without RA06. This allows an independent implementation to clone another detail flow or use a missing interface at integration.

Fix: add RA06 to RA07 dependencies for final UI completion (the route-window facts work can still start after RA02/RA03/RA05). Alternatively move shared details ownership into an earlier shared child and update all consumers, but adding the edge is much simpler and acyclic.

### D4 — RA02 category wire contract: reserved summit IDs collide with the obvious train extension

Existing `obc-formats/src/obcm.rs` reserves category 7 and subtype 19 for summits. `PoiCategorySet::ALL` currently derives a contiguous six-bit mask from `POI_CATEGORY_COUNT`; adding train as a seventh browsable category at ID8 without repairing that mask omits trains from Everything and includes the non-service bit. This is a known concrete integration risk, not a generic demand to spell out all implementation details.

Fix: keep summit category7/subtype19 intact, allocate Train station category8/subtype20, update category iteration/filter masks and validation so they derive actual browsable IDs rather than 1..count assumptions. Verify Everything contains all seven service categories and excludes summits; train roundtrips and appears in both nearby/corridor filters; Peak View still sees summit content. RA02 already covers pack/assemble/spec/vectors, so no new issue or layer is needed.

## Non-blocking confirmations and small clarifications

- Configured offset contract is appropriately narrow: GPS/BLE trust establishes UTC; configured `utc_offset_min` remains authoritative locale configuration. No global timezone service or auto-DST promise is needed. Offset update/day rollover re-evaluation plus Unknown when authority is absent is sufficient. An incorrectly configured offset produces correspondingly wrong local interpretation; do not claim geographic timezone detection. No extra permission flow is needed.
- Open/Closed/Unknown, pre-cap filtering, previous-day rollover, uncertain rounded/seasonal/truncated schedules and query errors are substantially covered. Preserve this conservative policy when implementing; current `is_open` alone is insufficient.
- Define Complete as completeness of the stated query inside known loaded map coverage, separate from whether the geometric radius/window extends outside that coverage. Current text/tests imply this; an explicit sentence prevents "complete empty" at a cut edge. Never infer last-water/service gaps.
- RA07 preserves generic/categorized waypoints, >16 paging, source/category filters, active/cross-window climbs, unknown elevation, accepted-journey axis and beyond-window next custom point. RA03 gives the needed validity/interval seam. This is coherent. No surface bar, meal inference or all-route climb list is introduced.
- RA12 preserves working detour capability rather than replacing it with a placeholder, makes Assistant ordinary startup, and retains former POI/filter capabilities. Consolidation boundaries and deletion scope are sound.
- RA10 depends on shared current-hours links and source identity; its private QID identity remains separate from service OSM identity. Ensure linked service hours describe the visited site/entrance, not a nearby unrelated business; the explicit mapping requirement in RA02/RA09 is the correct seam. No proximity-based hour transfer.
- RA01/RA13 provide actual source captures, normal shipping route/planner/storage paths, reproducible clock ports, offline commands, and a persistent card. Synthetic motion is labeled. This meets the real-data goal; no need for a new end-to-end harness. Device checks remain correctly pending.
- Resource plan is suitably bounded in ownership (buffers outside Screen, no country arrays) and respects the single final build/sweep policy. D1 is the missing practical compute bound; no unrelated memory system is necessary.

## Coverage and follow-up

Full review coverage: EPIC, RA02, RA06, RA07, RA12. Integration read: RA01, RA03, RA05, RA09, RA10, RA13; all child dependency headers checked. No substantive review claimed for the routing/landmark-specific algorithm interiors delegated to the other reviewers.

After D1–D4 edits, inspect only those changes and their propagated graph/acceptance clauses. If resolved without new conflicts, these reviewed issues are ready. Do not run another full review or rebuild for this prose-only correction.
