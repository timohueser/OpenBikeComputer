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

- "Find" holds one thing to find: a kind of place (`places.what`), a place by its name (`place.name`), or a fact of the route (`stretches.what`) with its least size (`min`). It also holds the action on a place, which is its label: "Find", "Route to", "Add a stop at" or "End the day at".
- "Where" holds the search area (`where`) and the radius. With no search area, the search uses the map view. It is off for a name: a name is searched near the place that its answer names, else in the map view, and keeps no search area of the search before.
- "When" holds the opening time (`open`). It is off for a fact of the route and for a name.

In the side panel, the row of the slots shows only while the rider searches: while the bar has the focus, while the field or a slot holds a value, and while the side panel shows the answer of the search (`searching` of [`PlannerApp.svelte`](../../builder/web/src/components/planner/PlannerApp.svelte)). The cross and the end of the search fold the row away.

At Enter, the server reads the sentence with the query model. The request of its answer fills the slots, and a slot that the answer leaves empty keeps its value. When the slots then give another request, the page searches again with that request, and the server reads no sentence; the list keeps the notice of the first answer. When the answer has two or more kinds, "Find" takes the nearest kind over all of them, else the first, and one line names the kinds that it leaves out. The words that the model ignored stay in the field.

While the rider types, the field marks the English words of single values: the actions, kinds, facts of the route, days and their parts, search areas and opening times. "route to" in "route to Konstanz" is the action "Route to", not the whole route. The lexicon builder of the query model writes these words to [`marks.json`](../../planner/search/query/marks.json), and [`marks.ts`](../../builder/web/src/lib/planner/search/marks.ts) gives each word its value. A sentence of only search areas and opening times, whose other words carry no meaning, changes its slots from these words, with no model. A sentence with "after", "between" or a bare "open" goes to the query model. Each term has one value: `lexicon.py marks` fails for a term of two values that its short list does not name. A suggestion and each change in a panel also search with the request of the slots and no model.

The model gives an action with its request type: `route` with `to`, `add_point` with `point` and `end_day` with `at`. When that place is a kind or a name, the request fills "Find" with the action and the place, and the search lists the places, as "Find" does. The area of `add_point` fills "Where", and the day of `end_day` gives the end of that day. "The next supermarket" and "the nearest supermarket" are the nearest one ahead on the route ("Nearest to me"): the model reads "next" as no field, so the page sets it from the marks of the sentence. The field marks "next", "nearest" and "closest" only before a kind of place: "next to", "next Sunday" and "the next 20 km" go to the model.

Nothing changes the plan before the rider presses a button. With an action, the first place has the one amber button of the action ("Route here", "Add as a stop", "End the day here"), and the other places have it as a quiet button. With "Find", each place has a quiet "Add as a stop" while the plan has a start and a finish, or is a loop. A button is off while the list loads or a press runs, and for a place where its change cannot apply, with the reason once, over the first such place: a stop or a day end with no start and finish and no loop, or a day end in the last riding day ("The last day ends at the finish."). With "Find" or "Add a stop at", a place that the plan has as a route point already shows that point and no button: "Start", "Finish", "Night of Day 1" or "Stop on Day 2". A press applies one change with `applyQueryChanges` ([`actions.ts`](../../builder/web/src/lib/planner/search/actions.ts), `placeChange`): a route from the rider, else from the start of the plan, as the server routes; a stop; or the end of the riding day in which the place lies. The list stays, and one line over it says what changed, with "Undo", which is the Undo of the planner and also brings back the way back to Explore and the shown week. After "Undo", the focus goes to the row of that place. After a press it does too, when the button that had the focus is off or gone; when the new list does not have the place, the focus goes to the heading of the list. The line goes at the next edit of the plan, a change of the slots or the end of the search. An action and its "Undo" keep the search when the plan changes between one day and more days.

Any other request that changes the plan fills no slot: `remove_point`, `split`, `join` and `reverse`, and an action that the slots cannot hold, such as a route with a start or a day count, or a stop every 30 km. The list shows it as one action row, with its description, its points and "Apply". A new search, such as one with the position of the rider, asks for that request again. The cross takes the change away and closes the search; unlike on Explore, it leaves no "Undo".

An area on the route has a base and a part. The base is the whole route, a day, or a stretch from km to km. As on the server, an open end of a stretch runs to the end of the route or the day, and a limit "after" or "before" a km mark, a day, the start or the finish narrows the base; a limit at a named place leaves the base to the server. The part is anywhere, the start, the middle or the end of the base: at an end, 20 % of the base and at least 10 km; in the middle, the middle 40 %. The request always sends such an area as its stretch (`where.along` with `from` and `to` in km from the start), so the list, the profile and the map show the same stretch. A day and a part that the query model reads become this area too. The server reads a part alone as a circle around one point, so the page searches again with the stretch.

The stretch of "Where" is also the selected part of the elevation profile and of the map. A drag on the profile and a tap on a stretch in the list, such as a climb, set the stretch of "Where", and a new stretch of "Where" marks the profile and fits the map. The part stays when the route changes, and the profile shows the share of it in the stretch that the profile shows. A stretch is limited to the route.

The panel of "Where" ([`WherePanel.svelte`](../../builder/web/src/components/planner/WherePanel.svelte)) draws the elevation profile of the route, with the colours of the days under it. A drag across the profile sets a stretch, and two range inputs move its ends, also with the keys. A tap on the profile or on a day button sets that day, and a second tap removes it. The stretch and its handles glide to a new day or part, and follow a drag at once. With a part, the profile shades the base lightly and the part fully. The profile scrolls the panel at a vertical swipe (`touch-action: pan-y`). Under the profile, "Within it" sets the part, and "Whole route", "Near me" and "Nearest to me" set an area by the place of the rider. "Nearest to me" is the next 20 km of the route ahead of the rider. While the map selects a point, "Selected point" searches near it. An area by a place off the route has no part: "Within it" is off, and the profile marks no place. The panel covers the list, so it shows the error of a search too, such as a position that is not available. While the panel is open, the page asks for "Find" along the whole route, and the profile shows the places as dots and the facts as bars. The ones in the stretch are marked, and a pointer that hovers names the one under it.

The panel of "Find" ([`FindPanel.svelte`](../../builder/web/src/components/planner/FindPanel.svelte)) has a switch for the action at the top, which takes two rows in a narrow panel, then the kinds of places and the facts of the route side by side. An action is on a place, so with an action the panel has no facts, and an action removes a fact from "Find". It is wider than the side panel and lies over the map. A chosen kind shows the kinds under it as one control, and a chosen fact with a size shows "At least". The line over the list ([`QueryResults.svelte`](../../builder/web/src/components/planner/QueryResults.svelte)) has the count of the places or stretches and the area that the search used. While the answer has a line under the bar (the kinds left out, a notice, or a change of the plan), an empty text field offers no values, because its list covers that line. A place row is the place and, under its distance, its action button, so a long button takes no room from the name; a press on the place opens its details. The pointer or the focus on the button lights the place on the map, as on the row.
