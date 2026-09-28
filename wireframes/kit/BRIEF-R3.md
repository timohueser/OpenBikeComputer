# Round 3 brief: rethink the phone planner

Read `kit/BRIEF.md` (frames, markup, maps, profiles, content sheet, copy rules) and
`kit/BRIEF-R2.md` (the owner's rules) first. This round is about the iPhone only. The website is
accepted as it is in the prototype (`~/Documents/OBC-plans/route-planner/prototype/index.html`).
Never write into the git repository.

## What failed on the phone (the owner's words, summarised)

Open the prototype's phone layout (in a desktop browser: "Prototype · example data" in the app bar
→ "Phone view"; or read `prototype/src/` and `prototype/check/out/phone-*.png`) to see it.

- The phone shows everything at once: map, box, chips, answer list, days, and a large amber "Send
  to device" button. On a small screen this does not work. "I can't make sense of how I would use
  it. It needs to be partitioned, so it is less all at once."
- When the keyboard is up (the rider edits the query), the screen holds a sliver of map, a sliver
  of answer, the keyboard and the big button. Too busy.
- The elevation profile has no proper place on the phone. It is important.
- "Send to device" is permanently prominent. In the planner it is not the primary action; that is
  the route preview page's job. The planner may offer "Save and send to device", but quietly.
- What worked: the sheet that the rider drags up.

## Principles for round 3

1. One job per state. The phone shows the map, or a list, or the profile, and at most two of them
   together. Never the map and a list and the keyboard at once.
2. Search is its own state. When the rider focuses the box, the search takes the full screen (the
   map hides); the box sits at the top, the understood chips under it, the live answer below. At
   rest the box stays in thumb reach (the owner's decision); on focus it moves up with the sheet,
   as in Apple Maps.
3. An answer returns to the map. When the keyboard goes down (Search key, or scrolling the list),
   the map comes back with the result markers, and a compact answer shows the chips and the list.
   Selecting a result shows its one action.
4. No permanent primary button. The navigation bar holds Back (or Done), the plan name, Undo and
   "···". "Save and send to device", Versions, Import, Offline areas, Continue on phone, Export GPX
   and the bike type and goal live in "···" (or in one quiet control). An answer's action ("End
   Day 4 here", "Use this route", "Apply") appears only while that answer is selected, and it is
   compact (one row, not a slab).
5. The profile has a home. It is a first-class state: full width, the day bands in the app's day
   colours (magenta #CC2A93, blue #2F6FB5, green #3B8A3F, violet #5B2FB0, repeating by riding day;
   dark: #E45CB5, #6FA8E8, #6CC47A, #A070F0), a scrubber that shows its point on the map, day ends
   that the rider drags on it, and the answers as ticks.
6. Calm: 44 pt targets, 17 pt body, at most two lines per row, neutral chips, lots of map.

## The same states in every direction

Draw each state as a whole phone (390 × 844, `.mk-phone`). Real content from `kit/content.md`.

1. `rest` — the Alps trip opened in the planner, nothing selected.
2. `search` — the box focused: the rider typed `campsites end of day 4`; chips and the live answer;
   the keyboard up (draw a plain iOS keyboard, as `box-states.html` does).
3. `answer` — keyboard down: the map with the campsite markers at the end of Day 4, the compact
   answer, Camping Caravaneige selected, its action "End Day 4 here".
4. `picker` — the "within 5 km" chip's picker open in the answer state, placed so the rider still
   sees the list change (the prototype's picker left room for one row only).
5. `profile` — the profile of the whole trip or of Day 4 with the day colours, the scrubber on
   Day 4 near the Col de l'Iseran and its point on the map, the Day 4 end handle.
6. `days` — the days list with the length bars in the day colours.
7. `route` — on the road: `Titisee` from here (Glottertal), the three options named by what they
   win, as differences; one selected; its action. (Plan "New route from Glottertal".)

## The three directions (how the phone is partitioned)

- **P1 · One sheet, clear states.** The map with one sheet (as the prototype, refined). Low detent:
  the box and one summary line. Medium detent: the profile, full width. High detent: the days, or
  the answer list. Focusing the box expands the sheet to full height and hides the map. The sheet
  never holds more than one of box-answer, profile or days at a time.
- **P2 · Three views.** A segmented control in the navigation bar switches three full-screen views:
  Map, Profile, Days. Search is a magnifier button in the navigation bar (and a field at the top of
  the Map view) that opens a full-screen search. An answer shows on the Map view with a compact
  docked panel at the bottom (docked to the screen edge, not floating). Most partitioned.
- **P3 · Map and a profile strip, with a tool bar.** At rest: the map, a compact profile strip
  (about 100 pt) docked at the bottom, and a bottom tool bar with Search, Days and "···" in thumb
  reach. Search opens full screen. Days opens a sheet. The strip grows into the full profile with a
  tap or an upward drag. Turning the phone to landscape shows the profile full screen.

## Mocks per direction (fragment `r3-N.html`, CSS prefix `pN-`)

Figures `pN-rest`, `pN-search`, `pN-answer`, `pN-picker`, `pN-profile`, `pN-days`, `pN-route`,
each `<figure class="mock phone">` with `<div class="fit" data-w="412" data-max="0.8">` and one
short caption. In your final message: 3–5 plain bullets on how the direction partitions the phone,
where it is strong and weak for the tent job, the road job and editing a trip on the phone, and any
rule you had to decide.
