---
title: Ride Assistant design study
description: Route brief and timeline interaction study.
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
