---
system: route-planning
kind: system
area: Apps
purpose: "Lets a rider plan a route on a map, in the browser or on the phone."
owns:
  - builder/web/src/lib/planner/**
  - builder/web/src/components/planner/**
  - builder/web/test-support/planner/**
  - companion-ios/Packages/OBCKit/Sources/OBCPlanner/**
  - companion-ios/Packages/OBCKit/Tests/OBCPlannerTests/**
  - companion-ios/PlannerNative/**
  - companion-ios/PlannerSearch/**
depends_on:
  - library
  - web-foundations
test_depends_on: []
status: draft
---

# Route planning

The body of this document is not written yet. Only the search has a section.

## The search

The search of the planner ([`PlannerQuery.svelte`](../../builder/web/src/components/planner/PlannerQuery.svelte)) uses the search bar of web foundations with three slots: "Find", "Where" and "When". Each slot holds some fields of a request of the query language ([`slots.ts`](../../builder/web/src/lib/planner/search/slots.ts), [`contract.json`](../../planner/search/query/contract.json)):

- "Find" holds one thing to find: a kind of place (`places.what`), a place by its name (`place.name`), or a fact of the route (`stretches.what`) with its least size (`min`).
- "Where" holds the search area (`where`) and the radius. With no search area, the search uses the map view. It is off for a name: a name is searched near the place that its answer names, else in the map view, and keeps no search area of the search before.
- "When" holds the opening time (`open`). It is off for a fact of the route and for a name.

At Enter, the server reads the sentence with the query model. The request of its answer fills the slots, and a slot that the answer leaves empty keeps its value. When the slots then give another request, the page searches again with that request, and the server reads no sentence; the list keeps the notice of the first answer. When the answer has two or more kinds, "Find" takes the nearest kind over all of them, else the first, and one line names the kinds that it leaves out. The words that the model ignored stay in the field.

While the rider types, the field marks the English words of single values: kinds, facts of the route, days and their parts, search areas and opening times. The lexicon builder of the query model writes these words to [`marks.json`](../../planner/search/query/marks.json), and [`marks.ts`](../../builder/web/src/lib/planner/search/marks.ts) gives each word its value. A sentence of only search areas and opening times, whose other words carry no meaning, changes its slots from these words, with no model. A sentence with "after", "between" or a bare "open" goes to the query model. Each term has one value: `lexicon.py marks` fails for a term of two values that its short list does not name. A suggestion and each change in a panel also search with the request of the slots and no model.

A request that changes the plan, such as a route to a place, fills no slot. The list shows its description, its points and "Apply change", and a new search, such as one with the position of the rider, asks for that request again. The cross takes the change away, and "Undo" asks for it again.

An area on the route has a base and a part. The base is the whole route, a day, or a stretch from km to km. The part is anywhere, the start, the middle or the end of the base: at an end, 20 % of the base and at least 10 km; in the middle, the middle 40 %. The request always sends such an area as its stretch (`where.along` with `from` and `to` in km from the start), so the list, the profile and the map show the same stretch. A day and a part that the query model reads become this area too. The server reads a part alone as a circle around one point, so the page searches again with the stretch.

The stretch of "Where" is also the selected part of the elevation profile and of the map. A drag on the profile sets the stretch of "Where", and a new stretch of "Where" marks the profile and fits the map. The part stays when the route changes, and the profile shows the share of it in the stretch that the profile shows.

The panel of "Where" ([`WherePanel.svelte`](../../builder/web/src/components/planner/WherePanel.svelte)) draws the elevation profile of the route, with the colours of the days under it. A drag across the profile sets a stretch, and two range inputs move its ends, also with the keys. A tap on the profile or on a day button sets that day, and a second tap removes it. The profile scrolls the panel at a vertical swipe (`touch-action: pan-y`). Under the profile, "Within it" sets the part, and "Whole route", "Near me" and "Nearest to me" set an area by the place of the rider. "Nearest to me" is the route ahead of the rider. While the panel is open, the page asks for "Find" along the whole route, and the profile shows the places as dots and the facts as bars. The ones in the stretch are marked.

The panel of "Find" ([`FindPanel.svelte`](../../builder/web/src/components/planner/FindPanel.svelte)) has the kinds of places and the facts of the route side by side. It is wider than the side panel and lies over the map. A chosen kind shows the kinds under it as one control, and a chosen fact with a size shows "At least". The line over the list ([`QueryResults.svelte`](../../builder/web/src/components/planner/QueryResults.svelte)) has the count of the places or stretches and the area that the search used. While the answer has a line under the bar (the kinds left out, a notice, or a change of the plan), an empty text field offers no values, because its list covers that line.
