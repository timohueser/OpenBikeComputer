---
title: Ride Assistant design study
description: Route brief, nearby places, and landmark interaction studies.
copy: ai
---

# What's next? A preview of the upcoming ride

The opt-in simulator study now includes the reviewed route brief and a scrollable timeline.
See the [simulator README](../../../apps/obc-sim/README.md#whats-next-study) for controls and limits.
The study uses fictional data. It does not summarize the active navigation route.

## Purpose

Start with a question a rider faces, then show the facts that help answer it. What's next gives
an overview of the next selected distance: terrain, the next climb, custom waypoints, and water
and resupply opportunities. Find a place helps the rider choose a destination. They sit beside
each other in Ride Assistant. A geographical map belongs in place or route-access details.

## Reviewed overview

Keep one compact screen with the range in its header, interval ascent and descent, the next
climb's start and statistics, a grade-colored elevation profile, the next custom waypoint, and
the next mapped water and shop. Use the Climb screen's grade colors. Omit the profile's Now/end
labels and the waypoint relationship sentence. Leave space between the groups.

A custom waypoint has the name supplied by the rider or route author. Do not infer that it is
lunch, rest, or accommodation. Its distance remains explicit if it lies beyond the profile window.
Do not add time to a presumed stop, whole-route climb counts, or service gaps to this overview.

Select **Explore ahead** to open the full available timeline for the selected window. Four visible
rows do not limit the number of entries. Back from details restores the selected row. Down + Back
opens the canonical filter drawer. Category and source filters affect the list, not the overview.
The prototype offers 5 km and 10 km; the production range presets remain open.

## Opening status across Ride Assistant

Exclude places known to be closed now. Include places open now and places with unknown hours.
Do not present unknown hours as confirmed open. Use current opening status, not predicted arrival
time. A closes-soon or unknown-hours note can appear in details where there is space.

The prototype uses supplied opening states. Production must resolve map schedules against current
local time. Missing schedules or unavailable time must not become a closed result.

## Nearby landmarks

Landmarks answers “What is this place?” or “What is near me?” Use a map with a selected card,
a name, a kind, and straight-line distance. Keep the map stable while the rider changes the
selection. Show short text that explains what the landmark is and why it is interesting.
Give longer text its own pages. Do not shrink it to fit beside the map.

The simulator uses four examples with text adapted from Wikipedia: Aare Gorge, Reichenbach
Falls, Gelmerbahn, and Dunlough Castle. Each has two short text pages. **Down + Back → Sources** opens attribution, the licence URL,
and the article URLs. Back restores the reading page. The locations and
access routes are fictional. The [study captures and attribution](../../assets/ride-assistant/landmarks-study/README.md)
identify the article revisions and licence.

**Visit** opens the shared visit preview. Straight-line distance helps identify a nearby place;
the preview supplies the prepared route cost. **Add stop** accepts the outbound and return legs
together. Browsing another place must not change this accepted visit or interrupt recording.

Aare Gorge, Reichenbach Falls, and Dunlough Castle each have a large ordered-dither
photo page. Up from the first text page opens the photo. Photo credits, source URLs, and licence URLs
are in **Sources**. The [photo study](../../assets/ride-assistant/landmark-photos/README.md)
compares rendering methods and storage cost. The simulator and a display-only board demo
use the same 216 × 240 RGB222 assets. The large format is the accepted choice. The device does not need a JPEG decoder.
The current assets are fixed examples. Map creation, image selection, and storage remain
future work. The [category proposal](../../assets/ride-assistant/landmark-selection/README.md)
uses fixed type rules, with no AI selection or per-place manual ranking. Lakes, mountains,
and glaciers are excluded. Passes are included, including entries without images.
[Random glacier and pass samples](../../assets/ride-assistant/glacier-pass-study/README.md)
show the content used for this decision. Production images should use basic lossless compression;
the codec and SD loading cost remain to be measured.

Next town is removed from the questions. It overlaps with What's next and Find a place, and
its purpose becomes unclear inside a city. Town names on the map remain a separate possibility.

## Easier route: wireframe proposal

Easier route compares alternatives from the current position to the same destination. The
[three wireframe concepts](../../assets/ride-assistant/easier-route/README.md) compare a goal list,
a cost table, and a map-first layout. The goal list is the initial recommendation, not an
accepted design. All costs are fictional. This screen is not implemented in the simulator.

Show the benefit and the cost of an alternative before acceptance. Less climbing, smoother
surface, and shorter distance are separate goals. A shorter route is not necessarily faster.
Time estimates need a suitable model before they can support a finish-sooner choice. The
proposal uses a separate review and an explicit **Use this route** action.

## Remaining implementation work

Production must preserve the full Up Ahead browser: category/source filters, generic and categorized
waypoint identity, route order, route distance and climbing, lateral offsets, stable selection,
passed-row behavior, and no-route and filtered-empty states. The next waypoint is not a new stop to
add again. Place details and accepted visit previews should be shared with Find a place.

Use actual route positions and elevation for the overview. Keep unknown elevation unknown; do not
replace it with flat terrain. Use a sensible profile scale and group crowded markers without
removing entries from the detailed list. During a visit, describe the accepted journey through the
return leg and remaining route. Do not treat arrival at the place as the route's finish.

Service gaps can help answer what happens if a rider passes an opportunity, but need a complete
search of the stated interval and access allowance. A bounded list cannot establish last water.
They remain outside this overview. A broader climb list belongs in the climb view.

The earlier [wireframes](../../assets/ride-assistant/README.md) remain as the design handoff.
