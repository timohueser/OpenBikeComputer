# RA03 — Supply honest shared route costs and coverage facts

Parent: #1734. Depends on RA01. Consumers are What's next, visit comparison and Easier route.

Implementation dependencies: [RA01 — #1736](https://github.com/timohueser/OpenBikeComputer/issues/1736).

## Existing owners and gaps

Reuse `obc-route/src/{reader,profile,climb,nav,splice}.rs`, graph `reader/nav.rs`, packer graph
classification and `integrate_edge_ascent`. Reuse Navigator's route-derived cache invalidation. Do
not create a second climb detector, effort model or elevation sweep per Assistant screen.

Current graph costs contain directional ascent and eight surface classes. Current OBCR output loses
surface attribution and cannot distinguish complete elevation from carried/zero-filled gaps. An
all-zero route may be genuinely flat or missing elevation. A weighted planner cost is not a physical
ascent/distance measurement.

## Deliverable

Define a small shared facts contract for an exact route revision and interval: distance,
ascent/descent, valid elevation coverage, surface-class distance and unknown-surface distance. Keep
facts tied to the same physical geometry and interval, including partial first/last edges and splice
seams. Store only facts needed by real consumers; put exact bytes/flags in OBCR_Spec and, where
graph validity changes, OBCM_Spec. Update all route producers/readers/transforms and contract
vectors together. Reserve the small waypoint provenance/accepted-plan descriptor fields owned by
RA04/RA05 in this same producer/consumer matrix; do not create competing format branches. No
migration layer and no new generic analytics framework.

- Preserve missing-elevation validity through import, DEM fill, output, reload, trim and splice.
  Reuse the existing ascent dead-band policy. Unknown segments do not create phantom climbs,
  flat profile spans or confident full-interval totals. Genuine zero elevation remains valid.
- Planner-created routes can retain known graph surface facts. For imported GPX, add bounded
  route-to-graph attribution using position, direction and continuity; ambiguous/off-network
  spans remain Unknown. Do not use visible road color or nearest geometry alone as proof.
  If measured attribution is incomplete, smoother-route availability must be conservative.
- Preserve these classes: unknown, paved, compacted, gravel, dirt, rough, cobbles, grass. Initial
  comparison's rough exposure is gravel/dirt/rough/cobbles/grass; paved/compacted is smoother.
  Keep unknown separate. Profile legality and suitability remain the existing router's rules.
- Expose interval ascent/descent and grade samples to the existing profile rendering path.
  Do not calculate grades from a min/max envelope. Define clipping and rounding once so card,
  table, overview and committed-route costs agree within the encoded format's stated precision.
- Freeze facts to exact map/route revisions and processing policy. Partial/outdated results are
  not interchangeable with complete facts. Use bounded streaming or existing cached profiles;
  no resident array proportional to route length.

## Acceptance and verification

A true flat route, no-elevation route and partially missing route remain distinguishable through
roundtrip and splice. Include a missing segment at each endpoint, a zero-length seam, directional
ascent, snap-partial edges, loop ambiguity, and a GPX near a parallel road. Check exact conservation
of distance/surface exposure over adjacent intervals and the defined ascent tolerance. Compare
committed route facts with preview facts from the same bytes. Use whole formats/route/reader/pack
suites and scoped Clippy/no_std. Use pinned real graph/terrain fixture suites for attribution. Full
comparable elevation coverage is required for a confident Less climbing saving or ascent tradeoff
cap. Partial values can be shown with an explicit unknown component, but cannot establish
eligibility. No ETA claim, new route optimizer or screen redesign is part of this issue.
