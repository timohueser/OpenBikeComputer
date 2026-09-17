---
title: Ride Assistant
description: Offline places, route facts, landmarks, and easier routes.
copy: ai
---

# Ride Assistant

Hold **Up + Select** for **500 ms** to find a place, inspect what comes next, compare easier routes,
or read about nearby landmarks. All four questions use installed offline data. The grey questions
remain inactive. Tap **Up + Select** to open the quick drawer with Bluetooth on/off. Peak View
stays in the main menu. The Detour command stays in the map context.
See the [simulator README](../../../apps/obc-sim/README.md#ride-assistant) for controls and data setup.

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
The range presets are 5 km and 10 km. Unit settings control how distances appear.

## Opening status across Ride Assistant

Find a Place hides places known to be closed now by default. Open **Down + Back** from the
category overview, category results, or **More places** to change **Hide closed places**.
Turn it off to include closed places. All categories and the paged browser use the same saved
preference. Places with unknown hours remain included and are not labelled open.
What's next continues to exclude places known to be closed.

Opening hours are information when a rider selects a destination. They do not block a route
preview or acceptance, including when a place closes during review. Landmarks remain visible
and available for visits regardless of opening status. Use current opening status, not predicted
arrival time.

The installed map supplies opening schedules. A trusted local clock resolves the current state.
Missing schedules, unsupported schedules, or unavailable time remain unknown.

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

Closed landmarks remain readable and available for **Visit**. Available hours describe the site;
they do not prevent a rider from viewing it from outside. Unknown hours do not claim that a site
is open. A site
without explicit mapped access for the selected bike profile remains information-only and shows
**Unmapped access**. The photo footer shows the same availability and keeps its page-navigation hint.
**Visit** opens the shared place detail and real route preview. A straight-line distance never
promises a rideable connection. The selected map, current position, profile, and mapped access
are checked before acceptance. Browsing does not change an accepted visit or start recording.

The [production evidence](../../assets/ride-assistant/implementation/ra10-evidence/README.md)
uses source-derived Swiss and West Cork content through the simulator and persistent card.
The earlier [landmark study](../../assets/ride-assistant/landmarks-study/README.md) and
[photo comparison](../../assets/ride-assistant/landmark-photos/README.md) remain design references.
The normal Assistant entry reaches this same path. Final recording acceptance and device loading
measurements remain pending the integrated simulator and hardware checks.

Next town is removed from the questions. It overlaps with What's next and Find a place, and
its purpose becomes unclear inside a city. Town names on the map remain a separate possibility.

## Current visit and restart

The visit preview shows the path from departure to the place and back to the original route in blue.
Its camera fits this part of the journey. The remaining original route stays in the stored journey
and appears in the ordinary route overview. A nearby stop does not zoom out to the journey finish.

While a visit is active, open **Assistant → Down + Back → Current visit**. The detail shows the
current leg. Select **Cancel visit** to leave the visit. Before departure, the original route returns
after the card confirms the saved change. After departure, review the real connector and select
**Use this route**. Back from the connector preview keeps the accepted visit. Back while a save is
pending only closes the view; it does not revoke the requested change.
If the card confirms that a cancellation was not saved, the visit stays active. Retry from
Current visit. A refused phase save keeps the last confirmed phase and retries the latest position.

The arrival card is informational. Select or Back closes it. Guidance and recording continue.
Rejoining the original journey clears an arrival card that is still open.

After a restart, a saved Assistant journey offers **Resume route**. This needs a fresh position
inside the saved route phase and exact source validation. A loop or crossing with more than one
matching position remains unavailable. Move to a clear part of the route and retry. Select resumes
guidance only after the saved change is confirmed. A refused save leaves Resume available.
It does not start recording. Back leaves navigation inactive. Open the
Assistant context drawer to reach the Resume card again. Ordinary routes do not resume on their own.
The recording recovery card makes a separate decision. Continue or Discard there keeps the saved
Assistant journey. Guidance stays inactive until you select Resume route.

## Easier route

The reviewed design is the map-first concept C. The simulator shows the real map with the
current route in magenta and the selected alternative in blue. Keep the camera stable while
browsing. Use start and finish symbols without map labels or a legend. The amber card shows
the goal and a large saving with a small pictogram.

Less climbing, smoother surface, and shorter distance are separate goals. Select opens a
current/new cost table. Lead with the positive saving, not the added distance or ascent.
Show the tradeoffs in the table. Omit the redundant same-destination sentence. Back preserves
the selection. **Use this route** is the explicit acceptance action and keeps recording active.

The shared planner searches each goal in sequence against the installed routing graph. Each choice
has measured distance, ascent, surface, and access facts. Missing elevation or surface attribution
stays unknown. A candidate must improve its stated goal and obey the selected bike profile.
The comparison freezes its route and map identity; changed inputs require a new search.

An accepted place visit keeps its required stop and return connection. Easier route stays unavailable
while that visit is active. A shorter route does not claim a shorter time: no finish-sooner goal or
new ETA model is supplied. The [earlier study captures](../../assets/ride-assistant/easier-route/README.md)
remain layout references; their synthetic alternatives do not ship.

## Discovery and limits

Find a place includes all map categories. Its context drawer offers **Results: 2 / 4 / 6**, with
four as the default. This saved preference applies to every category. A search calculates routes
to at most that many nearby candidates and, with an active route, that many candidates ahead.
It then ranks up to the selected number of recommendations. Fewer results reduce route work;
more results offer a wider choice. Neither choice changes the search area or guarantees a full
result page. Changing either search preference starts a fresh search for the current view.

**More places** opens the complete paged browser for the selected category. Back returns to its overview. The last calculated category stays cached until
another category replaces it or the Assistant closes, while its inputs remain valid. Accepting a
place route starts recording if no ride session exists. An existing ride continues in the same
session. What's next retains category and source filters, generic and categorized authored
waypoints, route order, route distance and climbing, lateral offsets, and stable selected identity.
An authored waypoint is information from the route; its detail cannot add it as a new stop.
Generated Assistant routes are internal and do not appear in saved Routes. Acceptance keeps their
bytes for navigation and recovery without adding a saved route. Imported routes remain available.
Place details and accepted visit previews use the same owner across all questions. During a visit,
**Assistant → Down + Back → Current visit** reopens its route and current-leg distance. Back
returns to the questions without cancelling the visit or changing the recording. If the accepted
route bytes are replaced, this view becomes unavailable. A covered visit view does not block a new
question. Large comparison totals use compact units so both columns fit at the normal font size.

The overview uses actual route positions and elevation. Unknown elevation does not become flat
terrain. During an accepted visit, the route includes the return leg and remaining journey; arrival
at the place does not become the route finish.

Service gaps can help answer what happens if a rider passes an opportunity, but need a complete
search of the stated interval and access allowance. A bounded list cannot establish last water.
They remain outside this overview. A broader climb list belongs in the climb view.

The earlier [wireframes](../../assets/ride-assistant/README.md) remain as the design handoff.

## Implementation plan

The [reviewed epic and child specifications](../../assets/ride-assistant/implementation/README.md)
define the production work for Find a place, What's next, Landmarks and Easier route. The plan
keeps the other questions as placeholders and removes Next town. It requires real offline inputs,
shared route acceptance and visits, deterministic landmark content, and focused simulator evidence.
The four questions use production data and navigation. Physical-device acceptance remains pending.
