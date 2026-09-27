# Route planner: click prototype and wireframes

Reference material for the route planner (#2236). This branch is an archive. It never merges.

- `prototype/index.html` is a clickable UI prototype of the chosen direction: map first, the
  query box with editable chips, example data, a fake sentence parser. The back end is not real.
- `wireframes/route-planner-wireframes.html` is round 1 (four layouts, the query-box states, the
  model choice). `wireframes/route-planner-round-2.html` is round 2 (three forms of the box, the
  chip pickers).
- `wireframes/kit/content.md` holds every place and figure the mocks use. All figures are examples.

## Open

- Laptop: open `prototype/index.html` in a browser. "Prototype · example data" in the app bar
  has "Phone view".
- iPhone on the same Wi-Fi: run `python3 -m http.server 8765 --bind 0.0.0.0` in `prototype/`,
  then open `http://<the Mac's address>:8765/` in Safari (`ipconfig getifaddr en0` prints it).

## Change

- Edit `prototype/src/`, then run `python3 build.py` in `prototype/`. It inlines the kit from
  `wireframes/kit/`.
- Wireframe pages: `python3 wireframes/page/build_page.py <out.html> [--shell shell-r2.html]`.
- The click check needs `npm i puppeteer-core` in `prototype/` and a local Chrome:
  `node check/run.js`.

## Differences from the product

The owner's notes after the first try: the site's header sits above the planner; a plan opens
from the list of routes and trips, not from a switcher; the left panel and the profile panel are
resizable; the profile zooms. The parser stands in for the model of #2238.
