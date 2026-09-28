# Route planner: UI prototype and wireframes

Reference material for the route planner (#2236). This branch is an archive. It never merges.
The record issue for this archive (what it is, the decisions, the open problems) is linked from
the research epic of #2236.

## What is here

| Path | What it is |
|---|---|
| `prototype/index.html` | The click prototype of the chosen design, for the website and the iPhone. Example data, a fake sentence parser, fake routing. Self-contained: open it in any browser |
| `prototype/screenshots/` | The final state, light and dark: website, phone portrait, phone landscape, plans page |
| `prototype/src/`, `prototype/build.py` | The prototype source; `build.py` inlines it into `index.html` |
| `prototype/BRIEF*.md` | The briefs that each prototype round was built from, in order: `BRIEF.md`, `BRIEF-FINAL.md`, `BRIEF-FINAL-2.md` |
| `wireframes/brainstorm.html` | The brainstorm page: rider jobs, best-in-class examples, what riders miss in other planners |
| `wireframes/route-planner-wireframes.html` | Round 1: four layouts, the query box states, the model choice |
| `wireframes/route-planner-round-2.html` | Round 2: three forms of the box, the chip pickers |
| `wireframes/route-planner-phone.html` | Round 3: three ways to split the phone, compared state by state |
| `wireframes/kit/` | The shared tokens, icons, sketch maps, the content sheet (`content.md`, all example places and figures) and the agent briefs of each round |
| `wireframes/*.html` (others) | The mock fragments that the round pages are built from |

## Open

- Laptop: open `prototype/index.html`. It starts on "Routes and trips". "Prototype · example data"
  in the tool bar has "Phone view" (with a rotate control for landscape).
- iPhone on the same Wi-Fi: run `python3 -m http.server 8765 --bind 0.0.0.0` in `prototype/`,
  then open `http://<the Mac's address>:8765/` in Safari (`ipconfig getifaddr en0` prints it).

## Change

- Prototype: edit `prototype/src/`, then run `python3 build.py` in `prototype/`. It reads the
  icons and sketch maps from `wireframes/kit/`.
- Wireframe pages: `python3 wireframes/page/build_page.py <out.html> [--shell shell-r2.html]`;
  round 3 first runs `python3 wireframes/page/make_matrix_r3.py`.
- Click check: `npm i puppeteer-core` in `prototype/`, then `node check/run.js` (uses the local
  Chrome).

## What is not real

The map is a hand-drawn sketch of three areas (the Black Forest, the Alps from Genève to Nice,
Day 4 close up). Routing, search, opening hours and the sentence parser are example data or word
tables. The real parser is the query box prototype of #2238 (tag `spike/query-parser-v2`).
