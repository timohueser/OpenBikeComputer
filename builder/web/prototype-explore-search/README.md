# Prototype: the day-hike search of the Explore page

A clickable mock of the search that issue #3703 designs. It is a throwaway prototype on the branch
`prototype/explore-search`. It never merges. The issue holds the decisions; this folder shows them.

## Open it

Open `index.html` in a browser. It needs no build and no server: `file://` works.

## What is real and what is not

- The hikes, car parks, lifts, places and numbers are invented, around Lucerne and Engelberg.
- The map is a still picture (`img/map.jpg`, © OpenStreetMap contributors). The mock zooms the
  picture; it does not load tiles.
- The route photos in `img/` are TrailView renders.
- "My location" puts the start point near Engelberg.
- The search, the filter counts and the order of the results run in `engine.js` on the sample data,
  the way the real page must behave.

## What to take from it

| Part | Where |
| --- | --- |
| Layout: filters on the left; map on top, results table below, with a drag handle between them | `index.html`, `shared.css` |
| Getting there: start point, then Car, Train (later), Lifts and Walk as one journey line | `engine.js`, `GROUPS.journey` |
| One walk limit from wherever the rider starts; the ways to reach a route | `engine.js`, `access()` |
| Limits and wishes (off, nice, must); best-match order | `engine.js`, `find()`, `ORDER` |
| Path difficulty as one range: upper end a limit, lower end a wish | `engine.js`, `gradeGroup()` |
| The empty list names the change that helps, with its count | `engine.js`, `emptyHtml()` |
| Map rules: which markers show, pins, "routes that pass here" | `engine.js`, `renderMap()` |

Copy behaviour and wording from here. Do not copy the code: the real page is Svelte, and it reads the
route catalog, not sample data.
