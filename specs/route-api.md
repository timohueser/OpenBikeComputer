# Route API

`POST /v1/route` on the [route service](../apps/route-server/README.md) calculates
routes. The native provider (`planner_router_request`) returns the same bytes for
the same request. Request and answer bodies are UTF-8 JSON.

## Request

| Field | Value |
| --- | --- |
| `points` | 2 to 64 ordered `[longitude, latitude]` pairs in degrees |
| `profile` | A profile ID from `GET /v1/region` |
| `pace` | Optional: `cycling_kmh` (1 to 80, default 19), `walking_kmh` (0.5 to 15, default 4.5) and `personal_multiplier` (0.25 to 4, default 1) |
| `alternatives` | Optional, default `false`. `true` asks for alternative routes |
| `alternatives_only` | Optional, default `false`. `true` asks for alternative routes and leaves out the primary route |
| `turnarounds` | Optional interior point indices where a reversal is deliberate |
| `start_position` | Optional leg position from an earlier answer. It pins the first point to that road position |
| `end_position` | Optional leg position from an earlier answer. It pins the last point to that road position |

The service rejects unknown fields.

## Answer

The answer is `{"routes": [...]}`. The first route is the primary route. With
`alternatives_only`, the answer holds only the alternative routes, and it can be
empty. `alternatives_only` overrides `alternatives` when both are `true`. When the
primary route fails, an `alternatives_only` request fails with that error, not
with an empty answer. Each route has these fields:

| Field | Value |
| --- | --- |
| `id` | 64-character lowercase hex route identity |
| `reason` | `primary`, `shorter`, `less_climbing`, `smoother` or `corridor` |
| `package` | Routing package identity |
| `profile` | Profile ID of this route |
| `coordinates_udeg` | Integer microdegrees: longitude and latitude of each point, flat |
| `elevation_dm` | Integer decimetres, or `null` where the height is unknown |
| `elapsed_s` | Integer moving seconds |
| `edges` | One run channel for each fact of an edge, by name |
| `legs` | `{"from_index", "to_index", "start", "end", "totals"}` for each pair of request points |
| `snap_truncated` | `true` when the service dropped road candidates for a request point |
| `totals` | `distance_m`, `ascent_m`, `seconds`, `surface_m`, `unknown_elevation_m` and `pushing_m`, all integers |

### Deltas

`coordinates_udeg` holds two integers for each point: longitude, then latitude.
The first pair is absolute. Each pair after it is the difference from the
previous point. Add the pairs in sequence and divide by 1,000,000 to get degrees.

`elevation_dm` and `elapsed_s` hold one integer for each point. Each value is the
difference from the previous value. The value before the first point is zero.
Add the values in sequence. Divide the heights by 10 to get metres. A `null`
height adds nothing: the next height is a difference from the last known height.
The first elapsed value is zero.

The encoder rounds the absolute value of each point and then calculates the
difference. Thus rounding errors do not add up along the route.

### Edges

An edge joins point `i` and point `i + 1`. A route with `n` points has `n - 1`
edges. `edges` maps each channel name to a list of runs `[value, edge count]`.
Each run gives one value and the number of consecutive edges with that value.
The run lengths of a channel add up to `n - 1`. A missing channel is `null` on
every edge. A client accepts a channel that it does not know.

| Channel | Value of an edge |
| --- | --- |
| `surfaces` | `Unknown`, `Paved`, `Compacted`, `Gravel`, `Dirt` or `Rough` |
| `pushing` | `true` where the rider must push the bicycle |
| `closures` | `null`, or a list of possible closures, each `{"kind", "condition"}` |
| `sac_scale` | The OSM `sac_scale` as an integer from `0` (`strolling`) through `1` (`hiking`, T1) to `6` (`difficult_alpine_hiking`, T6), or `null` when the way has none |

The router blocks a mode only where the rider surely
has no access. It uses an edge that is possibly closed for the mode that the
route uses on it, and reports it. A closure on a node, such as a gate, belongs to
the edges of the road that arrives at the node:

| `kind` | Source | `condition` |
| --- | --- | --- |
| `permit` | Access value `permit` | `permit` |
| `limited` | Access value `destination`, `customers`, `delivery` or `residents` | The value |
| `seasonal` | A conditional restriction that names only months, days or seasons | The OSM condition, such as `Nov-May` |
| `conditional` | Any other conditional restriction | The OSM condition, such as `wet` |
| `unclear` | An access value or a barrier that the router does not know | The value, or `barrier=VALUE` |

### Legs and totals

`legs[k].from_index` and `legs[k].to_index` are inclusive point indices. The
first leg starts at index 0. Each leg starts where the previous leg ends. The
last leg ends at index `n - 1`. `surface_m` gives metres for each surface, in
the order above. `seconds` is moving time. Distances and heights are metres.

### Leg positions

`start` and `end` are opaque strings. Each names the snapped road position of
a leg end: the road and the direction of travel on it. Where a point is not a
turnaround, the `end` of one leg equals the `start` of the next leg. A client
compares positions only for equality and sends them back unchanged.

A pinned point keeps only the road candidate at that position. Thus a request
for some legs of a trip joins the legs before and after it in the same
direction. The service ignores a position that is not a candidate of the
point, for example a position from a different `package`.

`legs[k].totals` has the fields of the route `totals`, for that leg only. The
sum of the leg totals is the route total. Only `seconds` can differ, by up to
0.5 s for each leg.

### Precision

Coordinates are exact: the routing engine stores microdegrees. Heights are
within 0.05 m of the engine value. Elapsed and total seconds are within 0.5 s.
The [vector](vectors/route-answer.json) gives one route before and after
encoding. The encoder test and each decoder test read it.

## Compression

The service compresses with brotli or gzip, as `Accept-Encoding` permits. It
prefers brotli.

## Errors

An error answer is `{"code", "message"}`. It never contains a substitute route.

The service attaches each point to the nearest road that the profile can use,
within 250 m. When none is that near, it uses the nearest one within 1 km. When
no route reaches the nearest road of a point that is not on a road, it uses the
next nearest road within 1 km. `no_snap` means that no such road is within 1 km.

| Code | Status |
| --- | --- |
| `invalid_request` | 400 |
| `no_snap`, `no_path`, `missing_region` | 422 |
| `cancelled` | 408 |
| `busy`, `limit` | 503 |
| `invalid_data` | 500 |
