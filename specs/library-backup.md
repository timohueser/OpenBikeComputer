# Library backup file

A library backup file is a ZIP file that holds the routes, the rides, the source
file of each ride, the library state and the settings of one browser. It can hold the photos of the
rides. It holds no covers: the browser renders them again.

- Version: `version` is `1`. A reader refuses every other version. The photos are entries and a
  field that a file can have or not, so they do not change the version.
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
A writer writes no file whose entries do not fit. The web app then offers a file with no photos:
no `photos` field and no photo entry.

| Entry | Content |
| --- | --- |
| `library.json` | The [envelope](#2-envelope), a UTF-8 JSON object |
| `rides/KEY.FORMAT` | The source file of the ride with the key `KEY`, unchanged. `FORMAT` is the `format` of the ride. |
| `photos/ID.jpg` | The copy of the photo with the id `ID`: a JPEG file |

## 2. Envelope

| Field | Value |
| --- | --- |
| `format` | `"openbikecomputer-library"` |
| `version` | `1` |
| `routes` | Array of [route records](#3-route-record), each with a unique `id` |
| `rides` | Array of [ride records](#4-ride-record), each with a unique `key` |
| `state` | [State](#6-state) object |
| `settings` | Object: the settings of the app |
| `photos` | Optional array of [photo records](#7-photo-record), each with a unique `id`. Without it, the file has no photos. |

A reader ignores the field `versions` of an older file.

## 3. Route record

| Field | Value |
| --- | --- |
| `id` | Non-empty string |
| `name` | String |
| `updatedAt` | Unix time in milliseconds of the last edit |
| `summary` | [Summary](#5-summary) with `days`, or `null` when the plan never had a calculated line |
| `trip` | Plan object of [planner-plan.md](planner-plan.md) |

A reader ignores the fields `open` and `revision` of a route record of an older file.

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
| `sent` | Object: for a route id, an object with `card`, the `storeId` of the card that holds the route; `days`, an array with the decimal `objectId` of the route of each riding day as last put, or an empty string before its first put; `crcs`, an array of the same length with the CRC-32 of the file of each day as last put or found on the card; `gone`, an array of the ids of `days` that the last listing of the card did not hold; `trips`, an array with the decimal `objectId` of the trip object of each part of 32 days, or an empty string for none; `tripKeys`, an array with the trip key in decimal of each part, stored before the put of its trip object, or an empty string; and `key`, a hash of the name and the plan of the route when its last send finished, else an empty string |
| `apart` | Array of ride keys: the rides that the rider took out of their group of rides on the same route |
| `deleted` | Array of objects with `key` and an optional `device`: the rides that the rider deleted |
| `nextMap` | Object or `null`: the map that the rider chose for the device and that is not sent yet. `regions` is an array of region ids of the map catalog. `boxes` is an array of objects with `minLat`, `minLon`, `maxLat` and `maxLon` in whole microdegrees. `light` and `dark` are the ids of the map styles for the day and for the night. `bytes` is the size of the download of the whole map |

## 7. Photo record

A photo record is the photo of one ride. The entry `photos/ID.jpg` holds its copy. The file holds
no small picture of a photo: a reader makes it from the copy.

| Field | Value |
| --- | --- |
| `id` | Non-empty string of letters, digits, `_` and `-`: the key of the ride, `-`, and a hash of the file of the photo |
| `ride` | The `key` of the ride of the photo |
| `time` | Unix time in seconds when the rider took the photo, or `null` for a photo that has its place on the ride from `place` |
| `place` | Optional array `[longitude, latitude]`: the GPS place of the photo file. Only a photo that has its place on the ride from it has this field: the photo file has no time, or the ride has no times. A record has `place` when, and only when, its `time` is `null`. |

The place of a photo on its ride is the sample of the ride nearest to `time`. With `place`, it is
the sample nearest to `place`, in the first pass of the ride within 100 m of it.

## 8. Load

A load first checks `format`, `version`, the fields of each record, the unique ids and keys, the
CRC-32 of each entry, the source file entry of each ride and the copy entry of each photo. It does
not check `trip`: the planner checks a plan when it opens it. When a check fails, the load changes
nothing. Only a value of `state` that fails its check does not stop the load: the load leaves that
value out.

A reader ignores an entry of the file and a field of the envelope that it does not know. So a
reader with no photos loads the routes and the rides of a file with photos.

Then one transaction adds:

- each route whose `id` is not stored;
- each ride whose `key` is not stored and is not a key of the deleted rides, with its source file.

The load takes `state` and `settings` only when the library is empty: no route, no ride and no
deleted ride.

Then the load adds the photos, one photo in one transaction. It adds each photo whose `id` is not
stored and whose ride is stored, also a ride that was stored before the load. It leaves out a
photo whose copy the browser cannot open. When a write fails, the photos before it stay, and a
second load of the file adds the others.
