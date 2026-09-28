# Final prototype round 2: our visual language, a plans page, a simpler phone sheet

The owner tried the final round on a laptop and an iPhone. The website layout and the functions are
accepted. This round changes four things. Read `BRIEF-FINAL.md` and `BRIEF.md` in this folder
first; everything they ask for stays unless this file changes it. Work in
`~/Documents/OBC-plans/route-planner/prototype/`; never write into the git repository at
`/Users/timo/Documents/OSM` (read from it freely).

## 1. Our visual language (website and phone)

The owner: the prototype strays from our aesthetic toward "the classic LLM-generated website":
neon accents and many half-transparent colour elements. It must look like our landing page and
our iOS app: solid colours, calm, the characteristic brown top bar, and a light mode that looks
good (light mode is the first check; dark mode must still work).

Study before you change anything:
- The landing page `docs/index.html` and the shared header `docs/assets/site-header.css` (the rust
  `#A4501E` bar, 56 px, cream text, the brand mark `docs/assets/brand/app-icon.svg`, the mono
  name), the tokens in `docs/assets/theme.css`, and the map builder `builder/app/src/` (the map app
  that already sits under the same header). Paths are in
  `/Users/timo/Documents/OSM/.claude/worktrees/requirements-category-filter-ac4402`.
- The iOS app: its direction contract `/Users/timo/Documents/OSM/.impeccable/surfaces/companion-ios.md`
  (colour roles: rust = on the device and the top band, amber = the one action, magenta on an
  amber casing = the planned route, olive captions, sage sketch ground, page #F4F2EB, the ledger
  for figures), the colours in `companion-ios/Packages/OBCKit/Sources/OBCUI/Theme/OBCTheme.swift`,
  and the real app screenshots `docs/assets/companion/*.webp` (convert with `sips -s format png`).

Then apply:
- **Website:** the site header (as `site-header.css`) sits on top of the planner, full width. The
  planner's own tool bar sits under it on the panel colour. Nothing else changes in the layout.
- **Phone:** follow the app's screens for the navigation bar and the sheet.
- **Solid colours.** Replace half-transparent colour fills and tints with solid colours from the
  palette: the selected row, the selected day band, the profile fills (a solid light shade per
  day colour, not an alpha of it), chip fills, answer highlights. No glows, no coloured shadows,
  no translucent coloured overlays. Magenta is for the route line and nothing else; the "what"
  chip is neutral like the others. Amber stays the one action.
- The day colours stay (the app's cycle); on the profile use solid light shades of them for the
  fills and the full colour for the line and the handles.

## 2. A plans page instead of the plan switcher

Remove the plan switcher from the planner completely: the owner does not switch between plans
while planning. Add a "Routes and trips" page before the planner, in the style of the app's lists
(the phone: a large title, rows with a small sage track sketch, the name, one olive figure line;
the website: the same list under the site header):
- the rows: "Alps: Genève to Nice" (trip, 10 days), "Schwarzwald Gravel, 3 days" (imported),
  "New route from Glottertal";
- one "New route" action.
Selecting a row opens the planner for it. The planner's Back (phone) or a "Routes and trips" link
in the tool bar (website) is the only way to another plan. The prototype may open on this page.

## 3. The phone sheet: two heights and a full-screen search

Replace the three detents with two, plus the search state:
- **Collapsed (the default):** a large map and, in the sheet, the elevation profile of the stretch
  the map shows, with the day colours and the draggable day-end handles, and one summary line
  above it. This is what the rider looks at most: the map with its profile.
- **Middle:** the profile, the search box beneath it, then one list: the days at rest, or the
  answer while an answer is open. The list scrolls inside the sheet. Only a drag on the handle or
  the sheet header changes the height; a scroll of the list never resizes the sheet.
- **Search:** tapping the box (or a search button in the collapsed sheet header, if you add one to
  reach search without dragging; keep it quiet) takes the full screen with the keyboard and hides
  the map, as now. The Search key returns to the middle height with the results on the map and
  as ticks on the profile.
- There is no full-height detent any more: it was almost the same as the search state.
- Landscape still shows the profile alone.
- The answer's action, the pickers (the sheet grows by the picker), the callouts, the day menu and
  the building tools work as in the last round; place them in the new two heights so nothing is
  lost. Where the middle height gets full, remove, do not shrink.

## 4. Zooming never hides other days

In the last round, zooming in on Day 4 switched to the close-up sketch map, which holds only
Day 4, so the other days disappeared. That is a prototype artifact, but the rule is: zooming never
hides a day. If it is cheap, draw the neighbouring days on the close-up map: both sketch maps are
linear in longitude and latitude (`kit/content.md` gives both formulas), so the Alps day paths can
be transformed into the close-up frame. If it is not cheap, leave it and say so.

## Checks and return

As before: one screenshot round (laptop 1440 × 900; phone 390 × 844 portrait and 844 × 390
landscape; light first, then dark), one scripted click run through the flows (the plans page,
both sheet heights, the list scroll without a resize, search, an answer, a picker, a day-end
drag, a callout, building), fix, run once more, stop. Serve once with `python3 -m http.server` and
confirm no console errors. Delete `node_modules` at the end.

Return 5–8 plain bullets: what changed, what you decided, and any problem you found.
