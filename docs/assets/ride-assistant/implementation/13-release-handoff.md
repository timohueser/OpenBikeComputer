# RA13 — Deliver a real-data simulator build and device-test handoff

Parent: #1734. Depends on RA12 and completion evidence from RA01–RA11. Start collecting evidence
throughout implementation; this issue is not permission to postpone real data or board feasibility.

Implementation dependencies: [RA12 — #1747](https://github.com/timohueser/OpenBikeComputer/issues/1747).

## Usable deliverable

Build regional map packs from RA01 sources through shipping bake/cut/assemble, with real POIs,
opening schedules, graph/terrain, landmark text/photos and provenance. Update the existing immutable
fixture registry, output hashes and recipes. Provide one documented `obc sim` scenario command for
Swiss riding, dense Monaco and West Cork access/landmarks. Provide a persistent card option so the
user can browse and accept real plans freely. No `--assistant-demo`, static costs, injected arrival
or live API dependency is allowed in these scenarios. Authored GPS replay is acceptable motion;
label it as replay, not as recorded field evidence.

Record actual visible source IDs/names and expected success or truthful unavailability. A real scene
need not provide every possible alternative. Validate article/image identity against pinned sources
and route geometry/costs against actual graph/terrain. Run after network access is disabled or
unavailable once packages are cached. A clean worktree must reproduce the scenario without the
author's second checkout, private files or manually edited summaries.

## End-to-end acceptance matrix

- Rural stop: choose a real POI, preview complete added cost, accept, replay arrival/dwell/return,
  rejoin and finish with one recording. Include a place nearer by air but farther by road.
- Dense city: more than 16 results, eligible places beyond closed-near records, More places,
  filters and stable Back while position/time changes. A failed search is not an empty result.
- Route brief: 5/10 km, active/cross-window climb, valid flat versus missing elevation, a named
  custom waypoint beyond the interval, source/category filters, and a window across an accepted
  visit. Preserve authored waypoint identity and ordering through replacement.
- Landmarks: source-derived natural/historical site, no-image pass, Irish castle, actual language
  fallback, full Sources, info-only access and a photo read failure with working text/Back.
- Easier route: genuine gain on a real competing road network, one/two/no alternatives, unknown
  surface/terrain, retained visit/waypoint constraints, exact reviewed bytes, and a second plan
  compared with the newly accepted journey.
- Lifecycle/storage: cancel, stale source, card/route/profile replacement, out-of-space, busy
  workspace, commit/release failures, restart at outbound/stop/return and ambiguous loop positions.
  Offer explicit Assistant Resume after exact source validation; do not invent ordinary route
  auto-resume. Check archive-proof preservation, uncertain commit fencing and avoidance-provenance
  refusal. Use actual executor/store traces; no fake owner response as the only evidence.

## Resource and validation budget

Each child runs whole focused suites and explicit captured-data fixtures. Registry changes pass `obc
suites check`. Root runs the final affected selection after integration; do not repeatedly mirror
all CI. Run at most one final UI snapshot sweep, then inspect named frames. No mutant tests or
repeated reviewer sweeps. Reviews have one substantive round plus delta checks for findings.

Root builds one final shipping image and compares it to `firmware/tools/resource_baseline.json`. Do
not rebuild the base or raise exact limits. Report App/resident/scratch/stack and flash deltas,
query/planner/decoder work budgets and actual photo byte distributions. Verify source holds <=5,
reservations <=2 and the existing arena limit with no permanent full-country/full-image buffers. A
resource blocker is corrected before declaring simulator work ready for device testing.

Recount Swiss geographic content through the production extractor: candidate identities, text,
licensed images, mapped approaches, omissions, compressed image totals and full map/index/credit
overhead. Explain differences from the P17-only study count; do not force its total or extrapolate
seven-image compression as a guaranteed country-wide ratio.

## Device session and handoff

Provide commit/branch, exact build command, ELF/map hashes, fixture commands and a short checklist
for tomorrow evening: physical buttons/readability, real SD photo latency, repeated selection,
map/route changes during work, arrival/dwell/rejoin with recording, cancellation/arena handoff, SD
failure/recovery, stack high-water and uninterrupted sensor/recording work. Prepare artifacts;
flashing/testing belongs to the scheduled user device session, not an invented simulator success.

Close with linked automated evidence, real-data simulator commands, known remaining limitations and
explicit physical checks still pending. Do not mark hardware acceptance passed until measured.
