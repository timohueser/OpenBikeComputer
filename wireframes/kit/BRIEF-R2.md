# Round 2 brief: map first, a calmer phone, a box that is more than a search field

Read `kit/BRIEF.md` first: its frames, markup contract, map and profile rules, content sheet and
copy rules all still apply. This file adds the owner's round-1 feedback, which overrides anything
in BRIEF.md that conflicts with it. Never write into the git repository.

## What the owner decided after round 1

- **Map first.** The map is the central element of the UI. Profile first is out.
- **The website of direction A is liked.** Open `dir-a.html` (figure `a-web`) and keep its
  structure: app bar, left panel about 360–380 px with the box on top, the map, the collapsible
  profile panel along the bottom of the map. Change only what this brief asks for.
- **Nothing floats over the map.** Answers, lists and cards live in the docked left panel on the
  website and in the sheet on the phone. Floating result cards over a map are bad UX (Komoot moved
  that way; the owner does not like it). Small map controls (zoom, layers, locate) may sit on the
  map. A popover that belongs to the panel may overlap the map edge; nothing else.
- **Days with day-length bars.** A trip shows its days as a list, each day with its length bar,
  as in the current trip editor and in `dir-d.html` figure `d-web-days`.
- **The phone must be calmer.** All round-1 phone mocks were too busy, unstructured and loud, and
  many targets were too small. Rules for every phone screen:
  - The full-screen map and one sheet with detents (as in the trip day editor). The box sits at
    the top of the sheet, in thumb reach.
  - One clear structure per screen: the box, then one content block (the route, the days, or one
    answer), then at most one amber action at the bottom of the sheet.
  - Targets at least 44 pt; the primary button 50 pt tall; rows at least 56 pt; body text 17 pt,
    secondary text 15 pt; never below 13 pt.
  - At most two lines of text per row. Drop metadata that the rider does not need at this moment.
  - Rows carry no buttons. A tap selects a row; the amber action at the bottom acts on the
    selection.
  - Colour is quiet: ink, olive captions, one amber action, the magenta route line. Chips are
    neutral (parchment fill, ink text); do not colour chips pink on the phone. No bands of
    coloured data rows on the phone unless the screen is about them.
  - Fewer things: no toolbar rows of small icons in the sheet; Undo sits in the navigation bar;
    everything else goes into one "···" menu.
- **The box.** Level 2 only: any wording of one request, never combined requests, never open
  questions. It is a very predictable, intelligent search, a quick way to get what would otherwise
  need several taps, especially on a phone. It must not look like a chat box.
  - It never shows words the rider did not type. Pointing at a day end, a point or a stretch puts
    a "where" chip inside the field, next to the cursor (`End of Day 4 · Valloire ×`), and the
    rider can type the rest after it.
  - **The chips are editable.** Each chip is one field of the request. A tap on a chip opens a
    small picker for that field and changes the request directly: "within 5 km" → 2 / 5 / 10 /
    25 km; "Campsites and lodging" → toggles for Campsite, Lodging, Hut, Shelter; "End of Day 4" →
    another day, or start / middle / end; "Open on Sunday" → the weekday; "Road" → the bike type.
    On the website the picker is a small popover under the chip, inside the panel; on the phone it
    is a small sheet or an inline row, with large targets.
- **The box must look central.** The owner's one critique of direction A: in the classic side
  position the box looks like a standard place and address field, so the rider does not see that
  it is the central tool. Each round-2 direction answers this in its own way (below). Keep it
  simple.

## The three round-2 directions (the twist on the box)

All three share everything above. They differ only in how the box shows that it is more than a
place search.

- **R1 · Classic, editable answer.** The box looks like a standard search field. The twist is in
  the answer: the understood chips appear under the field and each one is a control. Nothing else
  is new. This is the conservative option.
- **R2 · Scoped box.** The box always shows its scope as a chip inside the field, at the left:
  `Whole trip`, `Day 4`, `Here`, `Map view`. The scope follows the selection: select Day 4 in the
  day list, on the map or on the profile, and the scope becomes Day 4. The rider types
  `campsites end` and gets the end of Day 4. When the field is empty and focused, it lists three to
  five request starters for the current scope ("Places to sleep at the end of Day 4", "Water on
  Day 4", "Shops open on Sunday on Day 4") as plain rows. The scope chip is the pointing chip.
- **R3 · Quick requests.** Under the box, a fixed row of one-tap requests for the current
  selection: Sleep, Water, Shops, Bike shops, Gaps (icons with short labels, large targets), like
  the category buttons under the Google Maps search field. One tap builds the request and shows
  its chips, exactly as if typed. The box is for everything else. Most discoverable; it costs one
  row of space.

## Mocks per direction (fragment `r2-N.html`, CSS prefix `rN-`, figure ids as below)

1. `rN-web` — Website, at home, "ten days through the Alps". Day 4 selected. The answer to
   `campsites end of day 4` (or the direction's own way to get there) in the panel, the result
   markers on the map, the profile panel along the bottom with Day 4 highlighted. The editable
   chips visible; one chip's picker may be shown open.
2. `rN-phone-trip` — Phone, the trip at rest: "Alps: Genève to Nice" with the days list and their
   length bars in the sheet, the box at the top of the sheet in this direction's form, the map
   above. This is the calm base screen the owner will judge first.
3. `rN-phone-tent` — Phone, in the tent: the answer for places to sleep at the end of Day 4, one
   campsite selected, the amber "End Day 4 here" at the bottom.
4. `rN-phone-road` — Phone, on the road: `Titisee` from here (Glottertal), the three options named
   by what they win as differences, the amber "Send to device".

One short caption per mock. In your final message: 3–5 plain bullets on what the direction does
well and where it is weak, and any rule you had to decide.
