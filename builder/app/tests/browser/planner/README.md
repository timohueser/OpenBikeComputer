# Planner request budget

A browser journey counts the requests of one planning session: browse the map, plan a trip,
drag markers, switch bikes, search, and show places along the route. Three stand-in hosts answer
for tiles, fonts and the route service, so nothing leaves the machine.

```sh
npm run test:planner --prefix builder/app/tests/browser
```

`request-budget.json` holds one limit per request kind. The test fails when a kind exceeds its
limit. The log shows each phase of the session, so the growth is easy to find.
Cloudflare bills `basemap`, `places`, `terrain`, `glyphs` and `sprites`. The VPS serves the rest.

After a deliberate change, set each limit to the highest of three runs, times 1.25, rounded up,
plus 2. Update `basis` in the file if the rule changes.

`places` is the largest kind. It counts the tiles that the place index reads along the route. The
journey never zooms the map to zoom 14, so a zoom 14 tile belongs to the place index.
