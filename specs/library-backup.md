# Library backup file

A library backup file is a ZIP file that holds the routes, their versions, the rides, the source
file of each ride, the library state and the settings of one browser. It holds no covers: the
browser renders them again.

- Version: `version` is `1`. A reader refuses every other version.
- Writers: the web app (`builder/web/src/lib/library/backup.ts`).
- Readers: the web app.
- Code authority: `builder/web/src/lib/library/records.ts` (the record fields and their checks),
  `builder/web/src/lib/library/zip.ts` (the ZIP entries).
- Related: [planner-plan.md](planner-plan.md) (the plan object in `trip`),
  [flat-store-format.md](flat-store-format.md) (`StoreId` and `ObjectId`).

## Conventions

- Lengths, climbs and heights are metres. Times are seconds. Coordinates are WGS 84 degrees.
- An optional field is absent when it has no value.

## 1. Entries

Each entry is stored with no compression (method 0), with its CRC-32 and with UTF-8 names (flag bit
11). The file has no ZIP64 records, so it is smaller than 4 GiB and has fewer than 65,536 entries.

| Entry | Content |
| --- | --- |
| `library.json` | The [envelope](#2-envelope), a UTF-8 JSON object |
| `rides/KEY.FORMAT` | The source file of the ride with the key `KEY`, unchanged. `FORMAT` is the `format` of the ride. |

## 2. Envelope

| Field | Value |
| --- | --- |
| `format` | `"openbikecomputer-library"` |
| `version` | `1` |
| `routes` | Array of [route records](#3-route-record), each with a unique `id` |
| `versions` | Object: for a route id, the array of the versions of that route, newest first |
| `rides` | Array of [ride records](#4-ride-record), each with a unique `key` |
| `state` | [State](#6-state) object |
| `settings` | Object: the settings of the app |

A version is a version object of [planner-plan.md](planner-plan.md).

## 3. Route record

| Field | Value |
| --- | --- |
| `id` | Non-empty string |
| `revision` | Integer, 0 or more |
| `name` | String |
| `updatedAt` | Unix time in milliseconds of the last edit |
| `open` | Optional string: the id of the version that `trip` belongs to |
| `summary` | [Summary](#5-summary) with `days`, or `null` when the plan never had a calculated line |
| `trip` | Plan object of [planner-plan.md](planner-plan.md) |

## 4. Ride record

| Field | Value |
| --- | --- |
| `key` | The UTC time of the first point, `YYYYMMDDTHHMMSSZ`. Two files with the same key are the same ride. |
| `name` | String |
| `source` | `"device"` or `"file"` |
| `format` | `"obcride"`, `"fit"` or `"gpx"`: the kind of the source file |
| `summary` | [Summary](#5-summary) with `start` and `movingS` |
| `maxHr` | Number or `null`: the maximum heart rate that applies to the ride |
| `ftp` | Number or `null`: the threshold power in watts that applies to the ride |
| `trip` | Optional object: the trip of the device that the ride belongs to, with `key`, the trip key of the device as a decimal string; `day`, the day of the trip from 1; and `name`, the name of the trip, empty when the device no longer held the trip |
| `device` | Optional string `storeId:objectId`: the ride on the device that it came from |

## 5. Summary

| Field | Value |
| --- | --- |
| `line` | Array: longitude, latitude, longitude, latitude, …, each with at most 5 decimals |
| `heights` | Array with one entry for each point of `line`: whole metres, or `null` when not known |
| `bounds` | `[west, south, east, north]` of the whole line before simplification |
| `lengthM` | Number |
| `climbM` | Number |
| `highestM` | Whole metres, or `null` when no height is known |
| `seconds` | Number. For a route, the riding time that the routing gives. For a ride, the time from the first point to the last. |
| `bike` | `"road"`, `"gravel"`, `"touring"`, `"mtb"` or `"hiking"` |
| `days` | Route only: integer, 1 or more, rest days included |
| `start` | Ride only: Unix time in seconds of the first point |
| `movingS` | Ride only: the moving time |

`line` is the line simplified with Douglas–Peucker at 10 m. While more than 1,000 points remain, the
tolerance doubles, to 40 m at most. At 40 m the line keeps its points, so it can have more than
1,000 points.

## 6. State

Each field is optional.

| Field | Value |
| --- | --- |
| `active` | String or `null`: the id of the route that the planner opens |
| `waiting` | Array of route ids: the routes that wait to go to the device |
| `leaving` | Array of route ids: the routes to remove from the device |
| `sent` | Object: for a route id, an object with `device`, the route on the device as `storeId:objectId`, and `line`, the line key of the route when it was sent |
| `apart` | Array of ride keys: the rides that the rider took out of their group of rides on the same route |
| `deleted` | Array of objects with `key` and an optional `device`: the rides that the rider deleted |

## 7. Load

A load first checks `format`, `version`, the fields of each record, the unique ids and keys, and
the CRC-32 and the source file entry of each ride. It does not check `trip`: the planner checks a
plan when it opens it. When a check fails, the load changes nothing.

Then one transaction adds:

- each route whose `id` is not stored, with its versions;
- each ride whose `key` is not stored and is not a key of the deleted rides, with its source file.

The load takes `state` and `settings` only when the library is empty: no route, no ride and no
deleted ride.
