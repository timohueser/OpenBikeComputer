# Editable planner files

An `.obcplan` file is a UTF-8 JSON object. It contains one editable plan and its
saved versions. It has no browser database ID or database revision, and no routed
line: the planner calculates the line again from the points. Import gives the plan
a new ID and does not replace another plan.

## Envelope

| Field | Value |
| --- | --- |
| `format` | `"openbikecomputer-plan"` |
| `version` | `1` |
| `name` | Non-empty string, at most 120 characters |
| `trip` | Plan object |
| `versions` | Array of version objects, newest first |

A version has a unique non-empty string `id`, a UTC ISO timestamp `at` with milliseconds, an optional
string `name`, a display string `summary`, and a plan object `trip`. Import
calculates each summary from the plan. It rejects duplicate version IDs.

## Plan object

Required fields are `points`, `routeOrder`, `days`, `budget`, `target`, and `limit`.

| Field | Value |
| --- | --- |
| `points` | Array of point objects |
| `routeOrder` | Array of point IDs in route order, as specified below |
| `days` | Integer from 1 to 14 |
| `budget` | `days`, `distance`, or `hours` |
| `target` | Finite number, at least 1 |
| `limit` | Finite non-negative number |
| `mode` | Optional `route` or `trip` |
| `name` | Optional signed-route name |
| `bike`, `preset` | Optional activity and preset from the planner profile list |
| `startDate` | Optional valid calendar date in `YYYY-MM-DD` form |
| `loop` | Optional `true`; the start also ends the route |
| `restAfter` | Optional array of riding-day numbers from 1 to `days` |
| `restNames` | Optional array of strings |
| `splits` | Optional object from night numbers to progress from 0 to 1 |
| `climbTarget` | Optional finite non-negative number |

Each point has a unique non-empty string `id`, a string `label`, and a two-number
`coordinate` in longitude/latitude order.
Longitude is from -180 to 180. Latitude is from -90 to 90.

The point `kind` is `start`, `finish`, `pass`, `via`, `waypoint`, `detour`, `night`,
or `marker`. Optional fields are boolean `autoLabel`, string `placeKind`, a
coordinate `anchor`, and `leg` (`routed`, `straight`, `drawn`, or `transfer`). A drawn leg can
have a `drawn` array of coordinates between its endpoints. A drawn coordinate can have a
third finite number, the elevation in metres. A transfer leg is a straight
line that the rider does not ride, such as a train. It adds no ridden distance or time. Optional `turnaround`
is `true` when the point turns the route back. Any point can have an optional string `note`, such as the
description of a route file's waypoint.

A night has integer `night` from 1 to `days - 1` and ID `night-N`, where `N` is
that number. Night numbers are unique. Markers do not count as route points.
An open plan with two or more route points has one start and one finish. An
incomplete plan has zero points or one endpoint, with optional markers. A loop
has at least two route points, one start, and no finish.

The route goes from the start through the points of `routeOrder` to the finish.
A loop goes back to its start. `routeOrder` contains each route point that is not
the start or the finish one time.

## Import

Import validates the complete file before saving it. An invalid plan or version
rejects the file. The open plan stays unchanged.
