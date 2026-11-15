# Route pictures

A route picture (a cover) is one TrailView still image of a signed route from the air, with the
place of each point of the route's line in the image, so that a page draws the line on it. This
contract defines the keys in the bucket, the manifest and the cell files of a set of covers, the
key of a cover, and how a reader finds the cover of a route record.

- Version: manifest, cell file `format` `1`. A reader refuses every other `format`.
- Writers: `route-pictures.mjs` (`builder/web/tests/browser/trailview/`).
- Readers: the pages that list the signed routes of a region.
- Code authority: `builder/web/tests/browser/trailview/route-pictures.mjs` (the size, the date and
  the time of the light, the version of the camera rule), `builder/web/src/lib/covers/line.ts`
  (`pack`, the code of `eye`).
- Related: [route-catalog.md](route-catalog.md) (the records and their cells),
  [planner-release.md](planner-release.md) (the release and its credits),
  [home-views.md](home-views.md) (the same bucket and host).

## Conventions

- Each clock time is the wall clock of Europe/Zurich, which is the clock of TrailView, as `HH:MM`.
- A route record is a record of the route catalog with `line_udeg`. A long route has no cover.
- A hash is the start of a SHA-256 as 16 lowercase hexadecimal digits.

## 1. Keys

The keys are in the bucket `obc-maps`, which `https://maps.openbikecomputer.com` serves. That host
blocks a query string.

| Key | Content | `Cache-Control` |
| --- | --- | --- |
| `route-pictures/manifest.json` | The [manifest](#2-manifest) of the live set | `public, max-age=60, must-revalidate` |
| `route-pictures/sets/<set>/manifest.json` | The manifest of the set `<set>` | `public, max-age=31536000, immutable` |
| `route-pictures/sets/<set>/cells/9-X-Y.json` | The [cell file](#3-cell-file) of the grid cell `9-X-Y` | `public, max-age=31536000, immutable` |
| `route-pictures/covers/<id>/<key>.webp` | The [image](#4-cover) of the cover `<key>` of the relation `<id>` | `public, max-age=31536000, immutable` |
| `route-pictures/covers/<id>/<key>.json` | The [points](#4-cover) of that cover | `public, max-age=31536000, immutable` |

The writer never replaces a key other than the live manifest. It puts the manifest of a set there
to make that set live, after it checks in the bucket that each cover of each cell file of the set is
there. The public host is no proof: its cache keeps a file for a year. Sets share the covers: a
cover that the bucket holds is not sent again.

## 2. Manifest

A UTF-8 JSON object.

| Field | Value |
| --- | --- |
| `format` | `1` |
| `set` | The hash of the cell files of the set in ascending order of their cell IDs as text, and then of this manifest as JSON with an empty `set` |
| `release` | The ID of the planner release whose catalog the set was made for. It is information for the owner. |
| `look` | `C\|MM-DD\|HH:MM\|W\|H`: the version `C` of the camera rule, the date and the time of the light, and the size of each image. Now `1\|07-15\|10:00\|960\|640`. |
| `width`, `height` | `960`, `640`: the size of each image in pixels |
| `covers` | The count of covers of the set |
| `credits` | Array of strings: each credit of each cover, one time each. The page shows them with a cover. |

## 3. Cell file

A UTF-8 JSON object `{"format": 1, "routes": {"<id>": "<key>"}}`. It maps the relation ID of each
route record that has a cover and that names this cell in its `cells` to the key of its cover. This
holds for a grid release and for a regional release. A cell with no cover has no file.

## 4. Cover

The key of a cover is the hash of the text of `look`, `|`, and the numbers of `line_udeg` of the
record in decimal, joined with commas. So a cover belongs to one line and one look, and a new
render of the same line has the same key.

The image is a WebP of `width` × `height` pixels. The camera is `coverView()` of TrailView for the
line of the record, at the date and the time of `look` in the year of the render, sunny. A route
with no view has no cover: TrailView has no data there, or no view holds the line.

The points file is a UTF-8 JSON object with one entry in `x`, `y` and `eye` for each vertex of
`line_udeg`:

| Field | Value |
| --- | --- |
| `x`, `y` | The place of the vertex in pixels of the image, with one decimal. A place can be outside the image; the reader clips the line itself. `null` for a vertex with no place: it is behind the eye, or TrailView has no terrain data there. |
| `eye` | An integer from 0 to 255. `0`: the vertex has no place. Else the low 7 bits are a code `c` from 1 to 127 of the distance from the eye, `min(127, 1 + round(8 · log2(max(d, 10) / 10)))` for `d` metres, so the distance is about `10 · 2^((c − 1) / 8)` metres. Bit 7 (`128`) is set when the vertex is visible: inside the image, and not hidden by the terrain. |
| `credits` | Array of strings: the credits of the image |

## 5. Reader

1. The reader reads `route-pictures/manifest.json` for each planner release, with no query
   string.
2. For a route record, it reads the cell file of the first ID in the record's `cells`. A cell file
   that answers 404 has no cover.
3. It uses the cover only when the key in the cell file is the key that it computes from `look` and
   the record's `line_udeg`. So the covers stay through a new planner release, and a route whose
   line changed shows no cover until the next render.
4. When a cover file answers 404, the set is no longer live: the reader reads the manifest again.
