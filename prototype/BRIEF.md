# Click prototype brief: OBC route planner

Build a clickable UI prototype of the OpenBikeComputer route planner. It is a click dummy: the UI
must feel real, and the back end is fake (example data, a fake sentence parser, no routing, no
network). Its only job: let the owner try the chosen direction on a laptop and on an iPhone and
find any major issue before real work starts.

Working folder: `~/Documents/OBC-plans/route-planner/prototype/` (outside the repository; never
write into the git repository). Mock sources: `~/Documents/OBC-plans/route-planner/wireframes/`.

## The chosen direction (the visual spec)

Direction **R1 · Classic, editable answer**, map first. The mocks are the visual spec; match them:

- `wireframes/r2-1.html`: figures `r1-web` (website), `r1-phone-trip`, `r1-phone-tent`,
  `r1-phone-road` (iPhone).
- `wireframes/r2-chips.html`: the chip pickers (all figures). The owner approved them.
- `wireframes/box-states.html`: the round-1 answer states (not understood, partly understood,
  change proposal, applied with Undo, empty, needs a date, gaps). Use their logic, in R1's calmer
  style.
- `wireframes/kit/content.md`: all places, figures, routes and copy rules. Use only this data.
- `wireframes/kit/base.css`, `kit/icons.svg`, `kit/maps.svg`: tokens, icons, the sketch maps, the
  route lines and the profiles (read `kit/BRIEF.md` for their coordinate rules). Reuse them.
- `wireframes/kit/BRIEF-R2.md`: the owner's rules. Most important: the map is the central element;
  answers stay in the docked panel (website) or the sheet (phone) and never float over the map;
  the phone is calm (44 pt targets, 17 pt body, at most two lines per row, one amber action per
  screen, neutral chips, rows carry no buttons: a tap selects, the action bar acts); the box never
  shows words the rider did not type.

Both amber buttons on the website stay amber (Send to device in the app bar and the answer's
action). Do not change colours by state.

## Platform and delivery

- One self-contained `index.html` (inline CSS and JS, the kit's SVG symbols inline). It must work
  (a) opened from disk in Safari, Chrome and Firefox, (b) served over the local network and opened
  in Safari on an iPhone, and (c) published as a claude.ai Artifact. So: no network requests at
  run time; scripts from a CDN only if necessary and only from cdnjs.cloudflare.com or
  cdn.jsdelivr.net/npm with a pinned version; fonts from Google Fonts with a fallback. Keep the
  source editable: you may write `src/` files and a small build script that inlines them into
  `index.html`. Plain JS is fine; a tiny library such as Preact with htm is fine if it helps.
- Layout by width: at 700 px and wider, the website layout; below, the phone layout. On a laptop
  add one quiet control ("Phone view") that shows the phone layout in a 390 × 844 frame, so the
  owner can try both on the Mac. On an iPhone the phone layout is full screen
  (`viewport-fit=cover`, safe-area insets, no page zoom on input focus: inputs at 16 px or more).
- Both themes (the tokens already switch with `prefers-color-scheme`).
- A small label "Prototype · example data" in the app bar (website) or in the "···" menu (phone).

## Layouts

**Website (as `r1-web`):** app bar (OBC Planner, the plan name and its dates, Undo, Redo, Versions,
Import, Offline areas, Continue on phone, Export GPX, amber Send to device); left panel about 380
px with the box on top; the map; the profile panel along the bottom of the map, collapsible.

**Phone (as the `r1-phone-*` mocks):** navigation bar (back, plan name, Undo, "···" with the rest);
the full-screen map; one sheet with three detents (low: the box and one line; medium; high),
draggable by its handle and by the sheet header, with the box at the top of the sheet in thumb
reach; one amber action pinned at the bottom of the sheet when there is one.

**Map:** the kit's sketch maps (`map-alps`, `map-d4`, `map-bf`) with the route lines. Pan by drag,
zoom by wheel and pinch and by +/−. Switch between the maps by zoom level or by what is selected
(for example, selecting Day 4 shows `map-d4` framed on Day 4). Markers and labels live in the map
SVG in map coordinates. Keep it simple and smooth; this is not a map engine.

## Plans in the prototype

A plan switcher (the plan name in the app bar or navigation bar opens a short list):

1. **Alps: Genève to Nice** (trip, 10 days, dates set; content sheet). The main plan.
2. **New route from Glottertal** (Black Forest; "here" = Glottertal; bike Touring). For the road
   job, `Titisee`, `Kandel` and the road-bike Kandel route.
3. **Schwarzwald Gravel, 3 days (imported)**, only if time allows: the imported line with
   "Imported line · not re-routed" and the Hotel Krone visit.

## What must work (acceptance list)

At rest:
- The Alps trip shows the days list with length bars; tap or click a day to select it: the day is
  highlighted in the list, on the map (the map frames it) and on the profile.
- Tap a day end on the map or the profile: a "where" chip appears in the box (`End of Day 4 ·
  Valloire ×`), and the rider can type the rest after it.
- Undo and Redo work for every change the prototype makes (day ends, added points, applied
  changes).

The box (a fake parser, see below):
- While the rider types, the understood chips build up under the field; words not yet understood
  stay plain; the character count shows (80 maximum).
- Each chip opens its picker (as `r2-chips.html`): distance, place kinds, where (day, part, "Pick
  on the map or the profile"), weekday, bike type and goal, and "In this map view" (This map view /
  Along the whole route / a day). A choice applies at once; a tap on the chip closes the picker;
  a removed filter stays as a struck chip and a tap brings it back; after a chip edit the typed
  sentence stays in the field in a faint colour.
- The answers, each with its fixed actions:
  - **List of places**: rows with km, distance off the line and hours; a tap selects the row and
    highlights its marker on the map and its tick on the profile; the action bar acts on the
    selection ("End Day 4 here", "Add as stop"). "End Day 4 here" really moves the Day 4 end in
    the prototype: the days list, the bars, the profile and the map update, and one quiet line
    with Undo appears.
  - **One place** (`Kandel`): the card, "Route from here", "Add to route", and "Other places with
    this name".
  - **Route** (`Titisee`, `road bike route up Kandel starting here`): the options named by what
    they win, as differences; select one; "Send to device" shows a quiet confirmation line
    ("Sent to OBC"), no pop-up.
  - **Change to the route** (`end day 4 at saint-michel`, `via Hotel Krone St. Peter`): the
    proposal card with before → after; the ghost line on the map; Apply and Cancel; after Apply,
    one quiet line with Undo.
  - **Gaps** (`water gaps`): the list of stretches, each with "Add marker".
  - **Not understood** (`is the galibier worth it`): one plain line and the place search result.
  - **Partly understood** (`campsites with a pool end of day 4`): the struck chip, the rest answered.
  - **Empty** (`bike shop end of day 4`): the empty answer, "Search within 25 km", the nearest one.
  - **Needs a date**: switch the trip dates off in a small "Trip dates" control, then
    `shops open day 4` shows the need chip and "Set trip dates"; `shops open sunday day 4` works
    without dates.

The "where" rule (the owner's decision):
1. Words in the sentence that name a where always win (`day 4`, `end of day 4`, `along the route`,
   `here`, `near St. Peter`), whatever is selected or visible.
2. Otherwise, a where chip that the rider pointed at.
3. Otherwise, the map view. If the route passes through the view, sort the results by km along
   the route. If the view holds none, widen the area step by step and say so in one quiet line
   ("None in this view. 2 found within 8 km."). The chips show this where as "In this map view",
   and its picker can change it.
Selecting a day frames the map on that day, so `shops` then finds the shops of that day without a
special mode.

## The fake parser

A small keyword matcher, not a model. It must handle at least the sentences in the content sheet's
query-box table, plus these variations: word order ("day 4 campsites"), "tomorrow" when the trip
has dates and "today" is set to Day 3, German words for the common kinds and parts
(`Zeltplatz`, `Supermarkt`, `Ende`, `Tag 4`, `Mitte`), typos of one letter in kind words, numbers
("within 10 km"), and `end day 4 at a campsite` (a list of campsites at the end of Day 4 whose
action is "End Day 4 here"). Anything else: not understood, then place search over the example
places. Put the matcher in one small module with a table of words, so it is easy to read. It
stands in for the real model (issue #2238); do not make it clever.

## Out of scope

Real routing, real search, the model, accounts, persistence across reloads (a reset control is
enough), export, the animated QR code, offline downloads (a quiet "Offline areas" entry that shows
one static list is enough), versions (the entry can show one static list), settings beyond the
bike type and goal picker.

## Checks

- Look at the result once at 1440 × 900 and once at 390 × 844 (phone layout), in light and dark
  (headless Chrome: `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new
  --screenshot=...`). For the click flows, one scripted run is allowed: `npm i puppeteer-core` in the
  prototype folder and drive the installed Chrome through the acceptance list once; fix what
  fails; run once more; stop. Do not build a test suite.
- Serve it once with `python3 -m http.server` and confirm the page loads with no console errors.

## Return

The path of `index.html`, how to open it on the iPhone (the local network URL pattern), and 5–8
plain bullets: what works, what is faked, what you decided that the owner should know, and any
issue you found with the direction itself while building it (that is the point of the prototype).
