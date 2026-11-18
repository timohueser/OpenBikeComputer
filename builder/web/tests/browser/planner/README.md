# Planner request budget

A browser journey counts the requests of one planning session: browse the map, plan a trip,
drag markers, switch bikes, search, and highlight a kind of places. Two stand-in hosts answer
for the tile Worker and the route service, so nothing leaves the machine.

```sh
npm run test:planner --prefix builder/web/tests/browser
```

`request-budget.json` holds one limit per request kind. The test fails when a kind exceeds its
limit. The log shows each phase of the session, so the growth is easy to find.
The test also fails when, at load, a terrain or overlay request comes before the last basemap,
glyph or sprite request.
Cloudflare bills `basemap`, `places`, `overlays`, `terrain`, `glyphs` and `sprites`. The VPS serves the rest.

After a deliberate change, set each limit by the rule in `basis`.

`places` counts the places TileJSON and the zoom 11 place tiles that the place marks read for
the view. The marks read tiles only from zoom 16, or from zoom 12 with a highlighted kind, so the
session zooms in to zoom 12 after it highlights a kind, and then pans. `overlays` counts the route network and access TileJSON and its tiles.

## Tile pictures

The tiles of the map settings show crops of the planner map. To make them again, start the
development server in `builder/web` (`npx vite --port 5316`), then run from `tests/browser`:

```sh
node planner/tile-pictures.mjs --base http://127.0.0.1:5316 [--only cycling,relief]
```

The script writes `src/components/planner/tiles/<id>-<light|dark>.jpg`. It needs the network of
the planner release.
