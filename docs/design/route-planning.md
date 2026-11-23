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
- "Where" holds the search area (`where`) and the radius. With no search area, the search uses the selected part of the profile, else the map view. It is off for a name: a name is searched near the place that its answer names, else in the map view, and keeps no search area of the search before.
- "When" holds the opening time (`open`). It is off for a fact of the route and for a name.

At Enter, the server reads the sentence with the query model. The request of its answer fills the slots, and a slot that the answer leaves empty keeps its value. When the slots then give another request, the page searches again with that request, and the server reads no sentence; the list keeps the notice of the first answer. When the answer has two or more kinds, "Find" takes the nearest kind over all of them, else the first, and one line names the kinds that it leaves out. The words that the model ignored stay in the field.

While the rider types, the field marks the English words of single values: kinds, facts of the route, days and their parts, search areas and opening times. The lexicon builder of the query model writes these words to [`marks.json`](../../planner/search/query/marks.json), and [`marks.ts`](../../builder/web/src/lib/planner/search/marks.ts) gives each word its value. A sentence of only search areas and opening times, whose other words carry no meaning, changes its slots from these words, with no model. A sentence with "after", "between" or a bare "open" goes to the query model. Each term has one value: `lexicon.py marks` fails for a term of two values that its short list does not name. A suggestion and each change in a panel also search with the request of the slots and no model.

A request that changes the plan, such as a route to a place, fills no slot. The list shows its description, its points and "Apply change", and a new search, such as one with the position of the rider, asks for that request again.
