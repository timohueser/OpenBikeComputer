# RA11 — Calculate real easier routes and compare measured costs

Parent: #1734. Depends on RA03, RA04 and RA05. Preserve the reviewed map-first layout, stable
camera, centered saving/pictogram, current/new table and explicit Use this route action.

Implementation dependencies: [RA03 — #1738](https://github.com/timohueser/OpenBikeComputer/issues/1738), [RA04 — #1739](https://github.com/timohueser/OpenBikeComputer/issues/1739), [RA05 — #1740](https://github.com/timohueser/OpenBikeComputer/issues/1740).

## Candidate construction

Extend the existing `obc-route/src/nav.rs` request profile multipliers with a small per-request
objective. Do not mutate the rider's saved profile, add another router or replace graph legality.
Preserve forbidden highway/access classes and the distance lower-bound needed by the planner.

Generate a fixed bounded set of trials sequentially through RA04. Start with the current profile,
one shared baseline trial plus at most two trials per objective (seven total per refresh): stronger
directional ascent weight for Less climbing; stronger gravel/dirt/rough/cobbles/grass penalties for
Smoother surface; distance-led weights for Shorter ride while retaining all profile prohibitions and
existing non-ascent highway/surface multipliers. Lower only the ascent preference to its allowed
minimum; do not neutralize suitability penalties. This can yield no shorter candidate, which is a
valid result. Name and test the exact multiplier constants. Do not perform an unbounded penalty
search or claim global optimality from the existing bounded weighted planner. Keep one materialized
candidate and small result descriptors.

All trials use the same current origin/route occurrence, remaining destination and required anchors.
RA05 preserves all remaining authored waypoint metadata and existing on-route access anchors; an
accepted visit destination stays mandatory. No off-route annotation becomes a new visit by
inference. If preserving all constraints cannot fit the existing output bounds, return explicit
unavailable. During an active visit, Easier route is unavailable in v1; other information questions
remain accessible. Also refuse RA04's unresolved-avoidance context, including after reload. Do not
introduce persisted closure geometry or change an accepted stop merely to make an alternative
available.

## Eligibility and tradeoffs

Measure candidate and current remaining journey with RA03 against the exact preview bytes. Initial
meaningful gains: at least 50 m ascent saved, 500 m rough exposure avoided, or 500 m distance saved.
Less-climb and smoother trials may add at most max(2 km, 25% of remaining distance). Smoother and
shorter trials may add at most max(100 m, 25% of known remaining ascent). These are named first
release policy limits, not validated physiology. If ascent needed for a cap is unknown, do not claim
the cap is satisfied. Report unavailable or an explicit unknown comparison rather than guess.

Less climbing and every ascent cap require complete comparable elevation coverage. Smoother requires
measured reduction in defined rough exposure without increasing unknown-surface exposure; an unknown
interval cannot win by replacing known gravel. Shorter uses physical route distance, not time or
route cost score. A trial's changed weight is not evidence that it improved. Deduplicate identical
geometry/costs, keep genuine beneficial alternatives, and show one/two/no choices honestly. Never
pad all three cards with invalid or identical routes. Explain unavailable goals briefly in their
detail state.

The benefit headline shows the selected saving. Full tradeoffs stay in the aligned table. Fit the
map to the compared changed geometry with a stable camera; use current/proposed colors and symbols.
Back preserves selection. Accept uses RA04's exact-source and movement revalidation and keeps
recording continuous. Reopening compares the currently accepted journey, not the original mock.

## Acceptance and verification

Test profile bans under overrides, exact measured gain, threshold/cap edges, missing/partial ascent,
unknown surface, imported GPX attribution, duplicate outputs, exhausted/NoPath distinction, loops,
required annotations/waypoint capacity, active visits, stale previews and second acceptance. Use
real mixed-surface competing roads; no alternatives on Grimsel is a legitimate expected result. Run
whole route/App/host suites, captured graph fixture suites and named map/table/unavailable frames.
Delete synthetic shifted paths, constant cost tables and demo route-ID acceptance from normal
runtime. No fastest-route/ETA model, difficulty score or automatic rerouting in this issue.
