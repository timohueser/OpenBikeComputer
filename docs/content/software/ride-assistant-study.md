---
title: Ride Assistant design study
description: Proposed route brief, timeline, and interaction rules for discussion.
copy: ai
---

# What's next? A preview of the upcoming ride

Status: proposed information design and wireframes for discussion. No firmware changes.
The earlier map-over-POI-list proposal was rejected because it did not answer the broader question.

## The question

What will the next part of the ride demand, where are my planned stops, and what useful places
will I pass along the way?

Use one distance window for the whole page. Start with the next 10 km as an example, with 5 km,
10 km, 20 km, and the remaining route as possible ranges. These presets are open for discussion.
At the end of the route, label the actual remaining distance rather than extending past the finish.

## The landing page

Build a compact route brief that can be read without scrolling:

1. **Terrain and work:** ascent and descent for the window, plus the next climb's start, length,
   average gradient, and gain. An elevation strip shows the sequence of rises and descents.
   For the example: a 3 km climb at 7% starts in 2 km; it gains 210 m. The full 10 km contains
   340 m of climbing and 220 m of descent.
2. **The rider's plan:** the next custom waypoint, its distance, and its relation to the terrain.
   For the example: Lunch is 7 km ahead, after the descent. If several waypoints are inside the
   window, indicate that more are present; keep every available waypoint in the detailed view.
3. **Services:** a compact water and resupply summary for the same window. Put their positions
   on the elevation strip when legible. For the example: mapped water is 800 m ahead, before
   the climb; a shop is 6.4 km ahead, after the main climb.

This connects useful facts. A rider can see whether to get water before the climb and whether the
planned stop is before or after the demanding section. The device does not prescribe a stop.

The elevation strip represents distance along the ride. A geographical map is useful after selecting
a place or route change, where the rider needs to understand access. It is not the landing page.

## Glance first, explore second

Select **Explore ahead** to enter the available chronological timeline for this window. It includes
climbs, custom waypoints, and the places the current Up Ahead can show. It has no four-suggestion
limit. The existing bounded data sources still apply; do not imply complete coverage from a full
result buffer.

Keep category filters, source selection, custom-waypoint cues, distance and climbing along the
route, and side/offset hints. Add a way to browse climbs specifically. Define source selection so
an explicit Waypoints-only or Map-POIs-only view retains its current meaning. A climbs-only view
does not need a place-source selector.

Filters apply to the detailed timeline. A water-only list must not silently suppress climbs or
planned stops from the overall ride brief. Show the active scope clearly. Back returns to the brief
with the same distance window. Back from an entry's details retains the list's selection and scope.

A climb opens its terrain details. A custom waypoint opens its route-stop summary. A discovered
place opens the shared place details and visit preview. Access distance and climbing must be
calculated there; along-route distance and lateral offset are not full visit costs.

## Proposed controls

- On the brief, Up/Down changes the look-ahead distance, like zoom. The title changes with it.
- Select opens Explore ahead for that range. In the timeline, Up/Down selects entries as usual.
- Down + Back opens range and filter controls in the detailed view. Category and source choices
  remain available, including all existing service categories.
- Back returns one level. Keep the scope and selection stable while the rider browses.

The brief is informational: its service icons are not touch targets. The detailed timeline is the
place to select an event. The exact controls are part of this proposal, not a firmware decision.

## Deterministic content rules

Use structured route facts and a small set of templates, with no generated prose or effort score.

- An active climb: describe what remains from the current route position.
- One upcoming climb: show its start, length, average gradient, and gain.
- Several climbs: show their count and the first start; show the sequence in the profile and
  retain every available climb in Explore ahead. Do not use a vague hardest-climb ranking as
  the only way to decide what the rider sees.
- Mostly descending terrain: describe the descent and any rises. This does not claim easy riding;
  surface, wind, and technical difficulty may be unknown.
- Planned waypoints: use their route positions and supplied names. Do not infer the purpose of
  a waypoint from a guessed place identity. The example Lunch label belongs to the authored plan.
- Relationships such as Before the climb and After the descent require actual route ordering
  and a matching terrain interval. If the relation is ambiguous, show the distance alone.
- Water/resupply summaries show mapped opportunities in the window, not verified opening,
  stock, flow, or rideable access. Do not issue a last-water warning from a truncated query.

Use stable templates and a stable browsing snapshot so the brief does not keep rewriting itself.
The route code already has elevation profiles, interval ascent, and detected climb segments that
can support this exploration; this proposal does not implement the summary calculation.

## Crowding and missing information

Group overlapping service markers on the small profile. Keep planned waypoints identifiable;
if all cannot fit, use a count and leave all available items in the detailed timeline. The overview
is selective presentation, not deletion from the route or the browser.

Use a sensible vertical profile scale and actual grade/gain figures. A small rise must not look
like a mountain simply because each view stretches its height to fill the chart. The two fictional
10 km examples use the same 300 m vertical range.

If terrain data is unavailable, say so rather than drawing a flat profile or describing an easy
stretch. Distinguish a completed search with no mapped service in the window from unknown or
partial coverage. The word none in the 5 km wireframe assumes a complete search in that fictional
example; it is not the fallback for unavailable data.

If a climb continues beyond the selected window, show that continuation. Window totals stop at
the window boundary. If a planned stop is shown beyond it, label that fact explicitly, as in the
5 km example. During an accepted visit, describe the accepted journey including its return and
remaining route; do not mistake arrival at the shop for the end of the ride.

## Relationship to Find a place

What's next provides context for the upcoming ride. Find a place helps the rider choose a stop,
including useful alternatives away from the route. They share place details and the complete
visit-and-return flow. What's next absorbs classic Up Ahead; no extra main-menu or drawer POI
entry is added.

## What to judge in these wireframes

Does the brief answer the rider's practical next question before they need to browse? Is the
climb / planned-stop / service hierarchy useful across different rides? Is one Select into the
complete, filterable timeline an acceptable trade-off for a more useful landing page?

The examples have fictional values and simplified geometry. Their ascent, descent, and climb
figures are internally consistent; the layout and summary rules remain proposals for discussion.

## Review materials

See the [wireframes and handoff](../../assets/ride-assistant/README.md) for all five native-size
examples, the editable generator, and the separate simulator visit study.
