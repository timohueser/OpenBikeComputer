# Final prototype round: the phone as P1, the trip editor's functions, classic route building

This is the last UI round before a real prototype. The result must be a clickable prototype that
the owner tries on their iPhone and on a laptop to judge how intuitive it is. Read `BRIEF.md` in
this folder first (platform, delivery, data, the fake parser, the checks); its rules still hold
unless this file changes them. Work in `~/Documents/OBC-plans/route-planner/prototype/` and never
write into the git repository at `/Users/timo/Documents/OSM`.

The existing prototype (`src/`, `build.py`, `index.html`) is the starting point. The website layout
is accepted; change it only where this brief says so. The phone layout is replaced.

## 1. The phone becomes P1 · One sheet (the owner's pick)

The visual and interaction spec is `../wireframes/r3-1.html` (figures `p1-rest`, `p1-search`,
`p1-answer`, `p1-picker`, `p1-profile`, `p1-days`, `p1-route`) and `../wireframes/kit/BRIEF-R3.md`.
Read both. In short:

- Navigation bar: Back, the plan name, Undo, "···". Nothing else. "···" holds Save and send to
  device, Versions, Import, Offline areas, Continue on phone, Export GPX, the bike type and goal,
  Trip dates, and "Prototype · example data".
- One sheet with three detents; the detent is the switch. Low: the box and one context line (trip
  totals at rest, the scrubber reading on the profile, the answer count in an answer). Medium: the
  profile at rest, or the compact answer while an answer is open. High: the days list, or the full
  answer list. The sheet never shows more than one of these at a time.
- Focusing the box raises the sheet to full height and hides the map: the box with Cancel, the
  understood chips, the live answer, the keyboard. The Search key or a scroll of the list returns
  to the medium detent with the map and the result markers.
- An answer's action ("End Day 4 here", "Use this route", "Apply") is one compact row at the
  bottom of the sheet and exists only while a result is selected. There is no permanent primary
  button in the planner.
- A chip picker opens under its chip and the sheet grows by the picker's height, so the list and
  the markers stay in view.
- **Landscape shows the profile.** On a phone turned to landscape, the whole screen is the
  profile of the plan (the P3 figure `p3-landscape` in `../wireframes/r3-3.html`): every day in its
  colour, every day end as a handle, the scrubber with its reading. In the laptop's "Phone view"
  frame, add a small rotate control that shows the same.
- The route line on the phone map is drawn per day in the day colours (as on the website); the
  day-end markers stay ink.

## 2. The trip editor's functions stay (phone and website)

The current iOS trip day editor has these, and the planner must keep them (source:
`companion-ios/Packages/OBCKit/Sources/OBCUI/Trip/TripDayEditorView.swift`,
`.../LineMarkers/LineMarkerProfileView.swift`, `.../LineMarkers/LineMarkerMapView.swift`,
`.../Trip/OffLineStopSheet.swift` in `/Users/timo/Documents/OSM/.claude/worktrees/requirements-category-filter-ac4402`;
read their doc comments):

- **The profile shows the stretch that the map shows.** When the rider zooms or pans the map, the
  profile's window follows: it shows the part of the line inside the visible map (the part not
  covered by the sheet). At the whole-trip zoom it shows the whole trip. The profile itself does
  not zoom or scroll on its own; the map drives it. (On the website the same, with the profile
  panel along the bottom.)
- **Day ends move by a drag on the profile only.** Each day end is a handle on the profile with a
  44 pt grab band the full height of the plot. Dragging it moves the day end along the line; the
  line never changes; the two days' figures, their length bars and colours update live, and the
  colour changes exactly under the handle. A first movement that is more vertical than horizontal
  belongs to the sheet, not to the handle. The map's day-end pins are not draggable.
- **A place on the map can end a day.** Campsite and lodging pins show on the map once the map is
  zoomed in close enough (not at the whole-trip view). Tapping one opens a small callout anchored
  to the pin: its name, kind and distance off the line, and "End Day N here" (N = the nearest day
  end) plus "Add as stop". For a place off the line, "End Day N here" asks how the day reaches it,
  as the app's off-line stop sheet does: "Out and back" (a spur to the place and back) or "Through
  it" (the line leaves, passes the place and rejoins), each with what it adds (+km, +m).
- **Each day has its actions:** in the days list, a day's "···" (or a long press on its row) holds
  End at a stop, Rename, Split this day, Join with the next day.

## 3. Classic route building (phone and website)

The planner is a classic planner first. Building a route by hand must work well:

- **Add a point:** tap (phone) or click (website) on the map where there is no pin: a small callout
  anchored there offers "Add point" (adds at the end of the route) and the nearest place's name.
  Dragging the line itself inserts a point in that leg. A new point never reorders the others.
- **Point types:** tap a point (on the map or in the day's point list): its callout shows the type
  (Pass here, Shape, Visit, Sleep) as a segmented choice and "Remove point". Pass here is a ring,
  Shape a small dot (it bends the line and is not a stop), Visit a flag (the line goes there and
  comes back to the same point), Sleep a tent (a day end; choosing Sleep splits the day there).
  Removing a point joins its two legs and changes nothing else.
- **Segment mode:** between two points, the leg's mode is Routed, Straight or Drawn. On the
  website, the mode sits between the points in the day's point list; on the phone, tapping a leg of
  the line opens its callout with the mode. Drawn lets the rider drag a freehand line on the map
  for that leg.
- Routing is fake: a Routed leg may be a smooth curve between the points (it does not follow
  roads), a Straight leg is a straight dashed line, a Drawn leg is the rider's stroke. The figures
  (km, climb, riding time) update with a simple estimate. Say "example line" nowhere in the UI;
  the "Prototype · example data" label covers it.
- In the Alps trip, the days list expands a day (tap its chevron) to show that day's points with
  the segment modes between them. The plan "New route from Glottertal" starts with two points
  (Glottertal and Titisee after the road job) and is the easiest place to try building.
- Undo and Redo cover every building step.

## 4. Polish and consistency

- One grammar on both front-ends: the same chips, answer cards, copy, icons and point symbols. The
  website follows the same rule as the phone for the answer's action (a route answer's action is
  "Use this route"; Send to device stays in the website's app bar).
- Fix the issues found in the first build: an open picker leaves room for the list (the sheet
  grows); a chip tap with the keyboard up is never lost; the "None in this view. N found within
  X km." line is visible enough on the phone (for example as the context line of the low detent);
  map labels do not collide at a day end with a selected place (hide the lower-priority label).
- Calm and simple over complete: when a screen gets busy, remove, do not shrink. 44 pt targets,
  17 pt body, at most two lines per row, neutral chips, one action at a time.
- Keep the code small and readable. Delete the old phone layout code; do not keep both.

## Checks and return

As in `BRIEF.md`: one round of screenshots (laptop 1440 × 900; phone 390 × 844 portrait and
844 × 390 landscape; light and dark), one scripted click run through the flows of this brief
(`npm i puppeteer-core` for the run, then delete `node_modules`), fix, run once more, stop.
Serve once with `python3 -m http.server` and confirm there are no console errors.

Return: 5–8 plain bullets on what works, what is faked, what you decided, and any problem with
the design that you found while building it.
