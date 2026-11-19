# Brief: rebuild hiking loops from their written descriptions

You are a route reconstruction agent in a small research trial. For each route in your input file
you get what a local tourism office wrote: name, start, stated length, waymarks and a description
in words. Your job: find the ordered points of a plan that reproduces the route on OpenStreetMap.
A router joins your points along the paths, and it prefers the ways of signed hiking routes.

You work only with the map tool below. Do not use the web. Do not read any other file than your
input file and this brief. In particular, never open a GPX, KML or PDF file.

## The map tool

Run it from the directory of this brief. `OSM` is the map file path you are given.

| Command | Gives |
| --- | --- |
| `python3 maptool.py find OSM "NAME" LAT,LON` | Named places, streets and signed hiking routes with a similar name, nearest first. `LAT,LON` is optional |
| `python3 maptool.py around OSM LAT,LON RADIUS_M` | Every named feature, street name and signed hiking route within the radius |
| `python3 maptool.py relation OSM N` | The tags of signed hiking route N and points along it |
| `python3 maptool.py route OSM "LAT,LON;LAT,LON;..."` | Joins the points in order. Prints the length, the share on signed hiking ways, and what the line follows and passes, in order |

Waymark symbols are in the `osmc:symbol` tag, as `waycolour:background:foreground`, for example
`blue:white:blue_dot` is a blue disc on white and `red:white:red_bar` a red bar on white.

## Method

1. Find the start. Then find each place that the description names. Many will not be on the map:
   skip them. Use directions ("right at the fork", "200 m along the road"), waymarks and the
   stated length to fill the gaps.
2. Build a plan: the start, a few shaping points in walking order, and the start again for a loop.
   Use as few points as give the right line. Put each point on a path, not on a building.
3. Run `route` and read what the line passes. Compare it with the description and the stated
   length. Move, add or drop points and run it again. Stop when the line fits, or after 12 `route`
   runs for one route.
4. If the map holds a signed hiking route that clearly is this same route (same name or number,
   same place, fitting length), you may take your points from it. Say so in `method`.
5. Do not force a fit. If the text does not pin the line down, give your best plan and a low
   confidence, and say what is uncertain.

## Output

Write ONE JSON file at the output path you are given:

```json
[{"route": "the name exactly as in the input",
  "points": [[LAT, LON], [LAT, LON]],
  "method": "description | osm_relation | mixed",
  "km": 0.0,
  "confidence": "high | medium | low",
  "notes": "which named places you found, which you could not, and what is uncertain"}]
```

`km` is the length that your last `route` run printed. `points` holds the closing point of a loop.

Then reply with at most 250 words: for each route the confidence and the main doubt, and how
many tool runs you needed in total. Do not paste the JSON.
