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
The prototype offers 5 km and 10 km; the implementation plan retains these presets.

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

The production Landmarks view reads the installed map. It uses the current GPS position and
keeps a four-card page in nearest straight-line order within 10 km. **More landmarks** reaches
the next page. **Refresh** starts a new search. Distinct sites at the same position remain separate.
The selected source provides the name, language, text pages, and optional photo. No photo means
there is no photo page. Up from the first text page wraps to the photo when one is available.

**Down + Back → Sources** opens the full attribution for the selected site. Article and photo
credits share a paged drawer. Back restores the selected site and exact reading page. Sources
also remains available when a photo cannot be decoded. If a photo-credit page fails, Sources
returns to the article credits and omits the photo. The article remains readable. Photos use the
existing RGB222 frame preparation path; text and source pages do not decode an image while they draw.

Closed landmarks remain readable for identification. Current opening hours govern **Visit**:
a known-closed site cannot be accepted. Unknown hours do not claim that a site is open. A site
without explicit mapped access for the selected bike profile remains information-only and shows
**No mapped access**. The photo footer shows the same availability and keeps its page-navigation hint.
**Visit** opens the shared place detail and real route preview. A straight-line distance never
promises a rideable connection. The selected map, current position, profile, and opening hours
are checked before acceptance. Browsing does not change an accepted visit or start recording.

The [production evidence](../../assets/ride-assistant/implementation/ra10-evidence/README.md)
uses source-derived Swiss and West Cork content through the simulator and persistent card.
The earlier [landmark study](../../assets/ride-assistant/landmarks-study/README.md) and
[photo comparison](../../assets/ride-assistant/landmark-photos/README.md) remain design references.
The normal Assistant entry and final recording journey are separate integration gates.
Device loading measurements remain pending hardware acceptance.

Next town is removed from the questions. It overlaps with What's next and Find a place, and
its purpose becomes unclear inside a city. Town names on the map remain a separate possibility.

## Easier route

The reviewed design is the map-first concept C. The simulator shows the real map with the
current route in magenta and the selected alternative in blue. Keep the camera stable while
browsing. Use start and finish symbols without map labels or a legend. The amber card shows
the goal and a large saving with a small pictogram.

Less climbing, smoother surface, and shorter distance are separate goals. Select opens a
current/new cost table. Lead with the positive saving, not the added distance or ascent.
Show the tradeoffs in the table. Omit the redundant same-destination sentence. Back preserves
the selection. **Use this route** is the explicit acceptance action and keeps recording active.

The [study captures](../../assets/ride-assistant/easier-route/README.md) show all three choices.
The host supplies synthetic alternatives with the same endpoints and illustrative costs.
This does not implement routing, surface analysis, required-stop handling, or an ETA model.
A shorter route is not necessarily faster. Time estimates need a suitable model before they
can support a finish-sooner choice. The simulator blocks alternatives during a place visit;
production must define how rerouting and an accepted visit interact.

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

## Implementation plan

The [reviewed epic and child specifications](../../assets/ride-assistant/implementation/README.md)
define the production work for Find a place, What's next, Landmarks and Easier route. The plan
keeps the other questions as placeholders and removes Next town. It requires real offline inputs,
shared route acceptance and visits, deterministic landmark content, and focused simulator evidence.
The current screens remain prototypes until those implementation gates pass.
