# Editable planner files

An `.obcplan` file is a UTF-8 JSON object. It contains one editable plan and its
saved versions. It has no browser database ID or database revision. Import gives
the plan a new ID and does not replace another plan.

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

Required fields are `points`, `days`, `budget`, `target`, `limit`, and `variant`.

| Field | Value |
| --- | --- |
| `points` | Array of point objects |
| `days` | Integer from 1 to 14 |
| `budget` | `days`, `distance`, or `hours` |
| `target` | Finite number, at least 1 |
| `limit` | Finite non-negative number |
| `variant` | `valley` or `direct` |
| `mode` | Optional `route` or `trip` |
| `live` | Optional boolean |
| `name` | Optional signed-route name |
| `bike`, `preset` | Optional activity and preset from the planner profile list |
| `startDate` | Optional valid calendar date in `YYYY-MM-DD` form |
| `loop` | Optional `true`; the start also ends the route |
| `routeOrder` | Optional array of unique point IDs that exist in `points` |
| `restAfter` | Optional array of riding-day numbers from 1 to `days` |
| `restNames` | Optional array of strings |
| `splits` | Optional object from night numbers to progress from 0 to 1 |
| `climbTarget` | Optional finite non-negative number |
| `routing` | Optional selected alternative, as specified below |

Each point has a unique non-empty string `id`, a string `label`, a two-number
`coordinate` in longitude/latitude order, and finite `progress` from 0 to 1.
Longitude is from -180 to 180. Latitude is from -90 to 90.

The point `kind` is `start`, `finish`, `pass`, `via`, `waypoint`, `detour`, `night`,
or `marker`. Optional fields are boolean `autoLabel`, string `placeKind`, a
coordinate `anchor`, and `leg` (`routed`, `straight`, `drawn`, or `transfer`). A drawn leg can
have a `drawn` array of coordinates between its endpoints. A drawn coordinate can have a
third finite number, the elevation in metres. A transfer leg is a straight
line that the rider does not ride, such as a train. It adds no ridden distance or time. Optional `turnaround`
is `true` when the point turns the route back.

A night has integer `night` from 1 to `days - 1` and ID `night-N`, where `N` is
that number. Night numbers are unique. Markers do not count as route points.
An open plan with two or more route points has one start and one finish. An
incomplete plan has zero points or one endpoint, with optional markers. A loop
has at least two route points, one start, and no finish.

## Selected alternative

Export includes a routing line only when it is a picked alternative and its key
matches the plan. Other routes are calculated again. The line contains
`choiceId`, `key`, `profile`, `picked`, `coordinates`, `elevation`, `elapsed`,
`edges`, `stops`, `seconds`, `unknownSurfaceKm`, `pushingKm`, `unroutedKm`, and
`unknownElevationKm`.
Elevation and elapsed arrays have one entry per coordinate. Elevation can be
null. Elapsed seconds are finite, non-negative, and do not decrease. Each stop
has a point `id` and finite non-negative distance in kilometres. Stops match all
route points in route order. A loop repeats its start as the last stop. The first
distance is zero, and distances do not decrease. A line with distinct
coordinates has a positive last distance.

Edge arrays have one entry per segment. Their channels use the decoded values
of [the route API](route-api.md). Unknown channels stay with the line.
The line has no routing package or other alternatives. Import resets
`alternatives` to an empty array and `alternativesReady` to false.

Import validates the complete file before saving it. An invalid plan, version,
or selected routing line rejects the file. The open plan stays unchanged.
