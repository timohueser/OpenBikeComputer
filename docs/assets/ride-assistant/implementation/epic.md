# EPIC — Ride Assistant: four offline features with real map data

Turn the four reviewed simulator studies into production features: **Find a place**, **What's
next**, **Landmarks**, and **Easier route**. The rider must be able to explore them in the simulator
with real map, terrain, place, article, image, and routing data. The simulator must exercise the
same application, readers, planner, storage, and rendering path as the device.

This is the implementation plan for #1734. It supersedes that issue's older shortlist and unreviewed
layouts. The accepted prototype is on `codex/ride-assistant-landmarks`, at `d963d667`. Keep its
reviewed 240 × 320 layouts and four-button controls. The prototype is interaction evidence, not a
source of production costs, coordinates, arrival events, or routes.

## Scope fixed by design review

- **Find a place:** stable map and selected card, useful nearby/along-route results, actual
  routed distance and ascent to arrival, and added distance/ascent for the complete visit.
  Keep More places and all existing POI categories. Include train stations without timetables.
- **What's next:** 5 km and 10 km windows, grade-colored profile, interval ascent/descent,
  next-climb facts, next custom waypoint, and next water/shop. Explore ahead retains the full
  available timeline, category/source filters, generic waypoint identity, route order,
  lateral offsets, and stable selection. No inferred lunch stops or narrative advice.
- **Landmarks:** nearby physical sites, short offline article text, optional 216 × 240
  ordered-dither photo, and Sources in the Down + Back drawer. Use deterministic type and
  source rules. Keep passes. Exclude lakes, mountains, glaciers, industry, and settlements.
  Photos use basic lossless compression. A missing image does not remove useful text.
- **Easier route:** map-first selection among less climbing, smoother surface, and shorter
  distance; positive saving above the current/new cost table; explicit Use this route.
  Keep the centered pictogram/number and caption. Shorter does not mean faster. ETA is deferred.
- Hide places confirmed closed now throughout these features. Retain unknown hours and do
  not label them open. Use current opening status, not an arrival-time prediction.
- Preview is read-only. Easier route is unavailable during an active visit in v1. An accepted
  blockage that Assistant cannot reconstruct also makes route-changing actions unavailable. Accepting a prepared route changes navigation, not recording.
  A visit accepts the outbound path and continuation together. Arrival switches to the
  accepted continuation once, with an informational card and no new route computation.
- Keep Road blocked, Back on route, and Worth a detour as placeholders. Next town stays
  removed. Existing working detour/recovery entry points remain available until a separate
  replacement is implemented; this epic must not remove a working capability for a placeholder.

## Existing owners to reuse

The implementation must use the existing architecture, not a new assistant service stack.

| Responsibility | Existing seam |
| --- | --- |
| Route identity, matching, guidance, planning lifecycle | `firmware/obc-app/src/navigator.rs`, `navigator/following.rs`, existing Navigator planner operations |
| Planner and route transforms | `firmware/obc-route`, weighted distance/ascent and surface costs, current detour trim/splice |
| Exact source leases and temporary output | shared Acquire → Step → Commit → Release executor; sealed reservations from #1700/#1705 |
| Service discovery and route corridor | `firmware/obc-reader/src/reader/poi.rs`, `firmware/obc-app/src/corridor.rs` |
| Opening hours and local clock | `host/obc-pack/src/hours.rs`, `firmware/obc-reader/src/hours.rs`, app wall clock and configured UTC offset |
| Production place and timeline UI | `screen/poi_*`, `screen/up_ahead.rs`, reviewed `screen/assistant/*`, shared `screen/vocab` |
| Map creation and on-card formats | `host/obc-pack`, `host/obcm-assemble`, builder pipeline, `obc-formats`, `obc-reader`, `specs/` |
| Real-data inputs and replay | `fixtures/catalog.toml`, fixture tools, `obc sim`, `obc-sim`, shared device-core host harness |

Known gaps are explicit work: capped POI queries filter too late and lose completeness; partial
corridor failures can become empty results; overnight hours need day-rollover repair; normal route
planning currently adopts its result; route output lacks sufficient surface and validity facts;
landmarks are compiled mock assets, not map content. Do not duplicate these owners inside each
screen to bypass a missing capability.

## Child plan and dependencies

Child bodies are standalone implementation specifications, reviewed and linked as GitHub sub-issues. Each child includes its owner, decisions, acceptance,
verification, and deletion scope.

| ID | Deliverable | Depends on |
| --- | --- | --- |
| [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736) | Versioned real-data inputs and replay routes | — |
| [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737) | Complete, opening-aware place queries and service metadata | [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736) |
| [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738) | Shared route costs, surface exposure, and unknown-data semantics | [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736) |
| [RA04 — #1739](https://github.com/timohueser/OpenBikeComputer/issues/1739) | Explicit route preview and accepted-journey lifecycle | [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738) |
| [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740) | Real visit-and-continue plans and progress | [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA04 — #1739](https://github.com/timohueser/OpenBikeComputer/issues/1739) |
| [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741) | Find a place on real results and shared place details | [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740) |
| [RA07 — #1742](https://github.com/timohueser/OpenBikeComputer/issues/1742) | What's next from accepted route geometry and full timeline | [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740), [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741) |
| [RA08 — #1743](https://github.com/timohueser/OpenBikeComputer/issues/1743) | Deterministic landmark text and image preparation | [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736) |
| [RA09 — #1744](https://github.com/timohueser/OpenBikeComputer/issues/1744) | On-card landmark format, bounded reader, and photo decoder | [RA02 — #1737](https://github.com/timohueser/OpenBikeComputer/issues/1737), [RA08 — #1743](https://github.com/timohueser/OpenBikeComputer/issues/1743) |
| [RA10 — #1745](https://github.com/timohueser/OpenBikeComputer/issues/1745) | Landmarks UI from on-card data and shared visits | [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740), [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741), [RA09 — #1744](https://github.com/timohueser/OpenBikeComputer/issues/1744) |
| [RA11 — #1746](https://github.com/timohueser/OpenBikeComputer/issues/1746) | Real easier-route alternatives and measured comparisons | [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738), [RA04 — #1739](https://github.com/timohueser/OpenBikeComputer/issues/1739), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740) |
| [RA12 — #1747](https://github.com/timohueser/OpenBikeComputer/issues/1747) | Production Assistant entry, shared UI, and mock removal | [RA06 — #1741](https://github.com/timohueser/OpenBikeComputer/issues/1741), [RA07 — #1742](https://github.com/timohueser/OpenBikeComputer/issues/1742), [RA10 — #1745](https://github.com/timohueser/OpenBikeComputer/issues/1745), [RA11 — #1746](https://github.com/timohueser/OpenBikeComputer/issues/1746) |
| [RA13 — #1748](https://github.com/timohueser/OpenBikeComputer/issues/1748) | Real-data simulator release and device-test handoff | [RA12 — #1747](https://github.com/timohueser/OpenBikeComputer/issues/1747) |

Start the real-data lane immediately. Work on place queries, route facts, and landmark source
preparation independently. Integrate small working slices; do not defer all real-data testing to
RA13. RA04 and RA05 are the navigation dependency path. RA13 assembles evidence and a usable
release; it does not invent another application or fixture system.

## Shared implementation rules

1. Navigator owns accepted navigation and planner lifecycle. Screens request and render facts.
   Hosts execute bounded work. Do not add a second coordinator, route catalog, generic event
   bus, or persisted assistant database. RA04 adds a bounded checkpoint section to existing card Metadata
   and a descriptor to OBCR for explicit Assistant recovery; Recorder is not its state owner.
2. Use exact map/store/route identity and revision, operation tokens, and acknowledged release.
   A result produced against an old source or stale rider anchor cannot be accepted silently.
   Work cancellation and source replacement must release reservations, leases, and scratch.
3. Use existing card object types and map installation/transfer paths. Put offline landmark
   payloads in map content. Specify byte changes in `specs/`; update readers, writers, vectors,
   assembler and map fixtures together. Breaking formats are allowed; no migration layer.
4. Retain only bounded descriptors in App. Geometry, article text and compressed images live
   on the card. Use existing scratch/arena lifetimes; do not allocate full-country collections
   or add a permanent full-photo buffer. Limits remain enforced, not raised for convenience.
5. Distinguish Complete, Partial, Failed and Unavailable facts where they affect a rider claim.
   Unknown elevation is not zero ascent; unknown surface is not smooth; no graph match is not
   permission to draw a straight rideable connector. Report uncertainty without filling the
   overview with explanatory prose.
6. Preserve remaining authored waypoint annotations in order, using their existing projected
   on-route access anchors, and retain an accepted visit destination. Do not infer a lunch stop
   or force an off-route annotation coordinate into the path. No silent omission or rerouting. A planner that cannot preserve them must
   return an explicit unavailable result. Optional skipped-stop UX is outside this epic.
7. Freeze browsing geometry/order and selection while the rider explores. Recheck essential
   eligibility, current hours, exact sources, and the route start before acceptance. If facts
   changed materially, refresh the preview and require a new explicit acceptance.
8. Use real measurements and fixed policies. No learned ranker, AI-generated selection,
   invented effort percentage, fake ETA, or padded result list.

## Completion gates

- All four features work without `--assistant-demo`, static Stop arrays, synthetic route
  offsets, prefilled cost tables, scripted arrival, or compiled landmark photos.
- Existing POI categories, source filters and generic route waypoints remain reachable after
  consolidation. Other questions remain visibly inactive placeholders.
- Prepared visits and easier routes use the actual graph and storage commit path. Recording
  stays continuous across preview, acceptance, arrival, rejoin, cancellation and card failures.
- A versioned Swiss ride, dense Monaco ride, and West Cork walking/cycling-access example can
  be opened with documented `obc sim` commands. Their place coordinates, route costs, article
  identity, and pictures come from pinned real inputs. Offline replay works after sources
  have been downloaded. Artificial fault cases are clearly identified as tests.
- The Swiss landmark count is recounted through the production boundary/type/content pipeline.
  Report candidates, readable texts, licensed images, approachable sites, actual compressed
  bytes and missing-data reasons separately. The study's 1,703 candidates/1,482 P18 images is
  a sanity reference, not an exact production quota or a quality guarantee.
- Focused package/surface suites and source contracts pass. One final rendering sweep is
  enough. Root produces one final shipping image and compares it with the recorded resource
  baseline; no base rebuild. Keep exact resource gates and publish measured limits.
- The simulator build, source commit, fixture hashes, commands, known limitations, focused
  evidence, and device test checklist are recorded. Device testing is a separate acceptance
  gate for tomorrow evening; simulator evidence does not count as physical SD/stack evidence.

## Review and implementation handoff

The completed review covers every child and the parent for missing prerequisites, hidden duplication, data validity,
state ownership, constrained memory, and real-data acceptance. Record concrete findings and
resolutions. A follow-up review examines changed sections and unresolved findings only. An
unresolved correctness or ownership blocker prevents publication as a ready plan.

The implementation agent should read this epic, the child graph, the review ledger, AGENTS.md,
CONTRIBUTING.md, and docs/testing.md. Start from the reviewed prototype branch or integrate its
needed changes into a current develop worktree without discarding unrelated work. Check the actual
base before applying the file references: earlier issue handoffs are not current code. Keep commits
bounded and public documentation in separate docs commits. Never mark an issue complete based only
on a mock screenshot or a test stub that bypasses the actual executor.

Review status: **ready for implementation** after three independent domain reviews and targeted
checks of the fixes. No planning blocker remains. This is not implementation or device-test evidence.
See the [review ledger](https://github.com/timohueser/OpenBikeComputer/blob/codex/ride-assistant-landmarks/docs/assets/ride-assistant/implementation/review.md)
and [fresh-session handoff](https://github.com/timohueser/OpenBikeComputer/blob/codex/ride-assistant-landmarks/docs/assets/ride-assistant/implementation/README.md).
