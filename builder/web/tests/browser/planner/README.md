# Planner request budget

A browser journey counts the requests of one planning session: browse the map, plan a trip,
drag markers, switch bikes, search, and show places along the route. Two stand-in hosts answer
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

`places` counts the places TileJSON and the zoom 11 places tiles that the place index reads along
the route. `overlays` counts the route network and access TileJSON and its tiles.
