# Brief for all wireframe agents: OBC route planner

You make one part of a wireframe page for the OpenBikeComputer (OBC) route planner. OBC is an
open-source bikepacking GPS computer. The planner will live in the iOS companion app and on the
website. The owner will compare several layout directions and pick one. Other agents draw the
other parts in parallel, so keep to this brief exactly: the parts must look like one page.

Working folder: `~/Documents/OBC-plans/route-planner/wireframes/` (outside the repository; never
write into the repository). Read first, in this order:

1. `kit/content.md`: every place, figure, route and query you may show. Do not invent others.
2. `kit/base.css`: tokens and shared primitives. Use them; do not restyle them. Add only scoped
   CSS with your own class prefix (given in your task), inside one `<style>` block at the top of
   your fragment.
3. `kit/icons.svg`: icon symbols (`<svg><use href="#i-tent"/></svg>`). Use only these icons. If you
   need one more, draw it inline in the same style (24 × 24, stroke 1.8, round caps, currentColor).
4. `kit/maps.svg`: map, route and profile shapes (another agent draws it now; it may appear while
   you work). The IDs and coordinate rules are below, so you can build before it exists.
5. The precedent screens: `companion-ios/Packages/OBCKit/Sources/OBCUI/Trip/TripDayEditorView.swift`
   in `/Users/timo/Documents/OSM/.claude/worktrees/requirements-category-filter-ac4402` (read the doc
   comments only): the trip day editor puts the map full screen with a three-detent sheet that holds
   the profile and the day list; a day end moves only by a drag on the profile.

## Principles that constrain every screen

- A classic route planner first: map, points, bike type, profile. A few taps for a quick route,
  full control when wanted. On top, one box takes short sentences (80 characters). The box is an
  intelligent search over the route with fixed answer types (a list of places, one place, a
  route, a change to the route). It must NOT look like a chatbot or an AI product: no chat
  bubbles, no sparkles, no "Ask AI", no assistant avatar, no typing dots, no conversational copy.
- The box always shows how it understood the sentence ("Understood as" plus chips: `.understood`,
  `.chip.what`, `.chip.where`, `.chip.filter`, `.chip.off` for ignored words, `.chip.need` for a
  missing input such as a date). A route change is only a proposal until the rider taps Apply.
- One planner, two front-ends: the website (mouse, 1280 × 800) and the iPhone (touch, 390 × 844)
  have the same features and should look as similar as the input allows.
- Riding time, never clock times. Honest figures: known or estimated. Ask, do not push: no
  pop-ups, no modal warnings, at most one quiet line in a summary.
- The rider owns the line: an imported line is never re-snapped; moving a day end never re-routes.
- Trip dates are optional, easy to find, never required.
- v1 features that must have a visible home somewhere in each direction (not all on one screen):
  alternatives on an A→B route (2–3, named by what they win, shown as differences); point types
  (Pass here, Shape, Visit, Sleep); per-segment mode (Routed, Straight, Drawn); undo and redo;
  versions; import; riding time; days on one line; gaps on request; photos and sights; search;
  offline areas; continue on phone (animated QR); send to device; the bike type and goal preset;
  an advanced settings entry. Experiment: snow history on the profile.

## Visual rules

- Operate mode: this is a tool. Familiar, calm, dense where useful. The map and the route are the
  loudest things; chrome is quiet. Amber marks the one primary action per view. Magenta on an
  amber casing is the planned route. Olive (`--ink-soft`) is for captions.
- Website mocks use Atkinson Hyperlegible Next (`--sans`); phone mocks use the iOS system font
  (`.mk-phone` sets it) and iOS conventions (navigation bar, sheets with detents, 44 pt targets).
- No kicker or eyebrow labels, no section numbers, no gradient text, no glass, no emoji, no
  colored side-borders on cards, no nested cards, no fake status bar (the phone frame draws only
  the island), no lorem ipsum. Card radius 12–16 px. One elevation per element (shadow or border).
- Both themes must work: use only the tokens (`var(--…)`), never raw hex in your CSS.
- All copy follows `kit/content.md` → Copy rules. Short labels. No helper paragraphs.

## Frames and markup contract

Your fragment is an HTML file without `<html>`, `<head>` or `<body>`. It contains one `<style>`
block and a sequence of `<figure>` elements, one per mock:

```html
<figure class="mock web" id="a-web">
  <div class="fit" data-w="1280"><div class="mk mk-web">
    <div class="winbar"><i></i><i></i><i></i><span>openbikecomputer.com/plan</span></div>
    <div class="app"> … </div>
  </div></div>
  <figcaption>One short line: which job and state this shows.</figcaption>
</figure>
<figure class="mock phone" id="a-phone-tent">
  <div class="fit" data-w="412" data-max="0.8"><div class="mk mk-phone"> … <div class="home-ind"></div></div></div>
  <figcaption>…</figcaption>
</figure>
```

- The page scales each frame to its column with CSS `zoom`, so design at the true size
  (1280 × 800 and 390 × 844) in px. Everything inside a frame is absolutely or grid positioned
  inside that fixed box. Nothing may overflow the frame.
- Maps: draw one inline `<svg class="…" viewBox="…" preserveAspectRatio="xMidYMid slice">`
  that fills the map area. Inside it: `<use href="#map-bf"/>` (or `#map-alps`, `#map-d4`), then the
  route lines as `<use href="#route-…" class="rt-casing"/>` followed by `<use href="#route-…"
  class="rt-line"/>` (alternatives: `rt-alt-casing` + `rt-alt`). Choose the viewBox to frame the
  area you need (it may be a crop, e.g. `viewBox="300 100 600 420"`). Put every map-anchored mark
  (pins, day ends, result markers, labels that point at places) INSIDE that same svg in map
  coordinates, so they line up at any size: e.g. `<g transform="translate(704 321)"><circle
  r="11" style="fill:var(--panel);stroke:var(--ink);stroke-width:2"/><use href="#i-tent" x="-7"
  y="-7" width="14" height="14" style="color:var(--ink)"/></g>`. Map label text in the svg uses
  `paint-order: stroke` with a `var(--m-label-halo)` stroke. HTML panels, sheets and cards sit on
  top of the map as HTML.
- Profiles: an `<svg viewBox="0 Y KM H" preserveAspectRatio="none">` with `<use href="#prof-…-area"
  class="prof-area"/>` and `<use href="#prof-…-line" class="prof-line"/>`. x is the distance in km;
  y = 300 − elevation × 0.1 (so 0 m is y 300 and 3,000 m is y 0). Crop Y/H to the elevations you
  need (Black Forest: `0 150 KM 150`; Alps: `0 0 KM 300`). Day bands, gaps, snow and result ticks are
  `<rect>`s in the same km coordinates (use `vector-effect: non-scaling-stroke` on strokes). Put
  all text labels in HTML above or below the svg, positioned in % of km (never text inside a
  non-uniformly scaled svg). The `.band` rows under the profile use `<i style="left:x%;width:w%">`.

## Shape IDs in `kit/maps.svg` (all `<g>` or `<path>` inside `<defs>`, in their map's coordinates)

| ID | What | Coordinates |
|---|---|---|
| `map-bf` | Black Forest base map | 0 0 1000 700 |
| `route-bf-shortest`, `route-bf-leastclimb`, `route-bf-leastunpaved` | Glottertal → Titisee, three options | bf |
| `route-bf-kandel` | Denzlingen → Waldkirch → Kandel summit | bf |
| `route-bf-import` | the imported "Schwarzwald Gravel" line across the map | bf |
| `route-bf-krone` | the detour from the imported line to Hotel Krone, St. Peter and back | bf |
| `map-alps` | Genève → Nice overview | 0 0 700 900 |
| `route-alps-d1` … `route-alps-d10` (no d6) | one path per riding day | alps |
| `map-d4` | Day 4 close-up, Val d'Isère → Valloire | 0 0 1000 600 |
| `route-d4` | Day 4 line | d4 |
| `prof-alps-area`, `prof-alps-line` | whole trip, km 0–634 | profile |
| `prof-d4-area`, `prof-d4-line` | Day 4, km 0–104 | profile |
| `prof-bf-titisee-area`, `prof-bf-titisee-line` | Shortest, km 0–34 | profile |
| `prof-bf-kandel-area`, `prof-bf-kandel-line` | km 0–20.8 | profile |
| `prof-bf-import-area`, `prof-bf-import-line` | km 0–312 | profile |

## Looking at your work

Build a preview with `python3 kit/build.py /tmp/<you>.html <your fragment>` and screenshot it with
`sh kit/shot.sh /tmp/<you>.html <out.png> 1400 <height> light` (and once with `dark`). Look at
the result once in each theme, fix everything in one pass, look once more, stop. Do not loop.

## What to return

Your fragment file path, and in your final message: 3–6 plain bullets on what each mock shows and
any design decision the owner should know about (plain English, no jargon). Do not write any other
files.
