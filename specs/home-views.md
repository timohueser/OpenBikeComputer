# Home views

The home views are TrailView still images of a few viewpoints. The Home page of the web app shows
one of them, by the day and by the hour of the visitor. This contract defines the keys in the bucket,
the manifest of a set of views, and how the page chooses an image.

- Version: manifest `format` `1`. A reader refuses every other `format`.
- Writers: `home-views.mjs` (`builder/web/tests/browser/trailview/`), from the list
  `home-views.json` beside it.
- Readers: the Home page of the web app.
- Code authority: `builder/web/tests/browser/trailview/home-views.mjs` (the sizes, the slots and
  their times).
- Related: [planner-release.md](planner-release.md) (the credits come from the release).

## Conventions

- Each clock time is the wall clock of Europe/Zurich, which is the clock of TrailView, as `HH:MM`.
- Coordinates are WGS 84 degrees.

## 1. Keys

The keys are in the bucket `obc-maps`, which `https://maps.openbikecomputer.com` serves. That host
blocks a query string.

| Key | Content | `Cache-Control` |
| --- | --- | --- |
| `home-views/manifest.json` | The [manifest](#2-manifest) of the live set | `public, max-age=60, must-revalidate` |
| `home-views/<set>/manifest.json` | The manifest of the set `<set>` | `public, max-age=31536000, immutable` |
| `home-views/<set>/<id>/<slot>.webp` | The image of the view `<id>` in the slot `<slot>` | `public, max-age=31536000, immutable` |

The writer never replaces a key under `home-views/<set>/`. Only the live manifest changes: the
writer puts the manifest of a set there to make that set live, after it checks in the bucket that
each image of the set is there. The public host is no proof: its cache keeps an image for a year.

## 2. Manifest

A UTF-8 JSON object.

| Field | Value |
| --- | --- |
| `format` | `1` |
| `set` | 16 lowercase hexadecimal digits: the start of the SHA-256 of the images and of this manifest with an empty `set`. A changed image, name or credit gives a new id. |
| `date` | `YYYY-MM-DD`: the date of every image of the set |
| `width`, `height` | `2560`, `1440`: the size of each image in pixels |
| `credits` | Array of strings: each credit of each image, one time each. The page shows them with the image. |
| `views` | Array of [views](#3-view), each with a unique `id` |

## 3. View

| Field | Value |
| --- | --- |
| `id` | Lowercase letters, digits and hyphens: the folder of the images |
| `name` | String: the name of the viewpoint |
| `region` | String: the region of the viewpoint |
| `lat`, `lon` | The eye |
| `sunrise`, `sunset` | `HH:MM` on `date` |
| `night` | `true` when the view has a `night` image |

## 4. Images

Each image is a WebP of `width` × `height` pixels. A view has the slots `morning`, `noon` and
`evening`, and `night` when `night` is `true`.

| Slot | Time of the image |
| --- | --- |
| `morning` | One hour after sunrise |
| `noon` | The time when the sun is highest |
| `evening` | One hour before sunset |
| `night` | 23:00. The sun is at least 6° under the horizon then. |

The writer refuses a view where the sun does not rise or does not set on `date`, where the sun is
up at midnight on the clock, or where the day is shorter than 4 hours. It uses only images of the
active production release.

## 5. Choice on the page

1. `D` is the number of days from 1970-01-01 to the local date of the visitor. The page shows
   `views[D mod count]`, where `count` is the length of `views`.
2. The page compares the local time of the visitor with `sunrise` and `sunset` of that view, as if
   the visitor stood at the viewpoint. In this rule, noon is the middle between `sunrise` and
   `sunset`.
   - Before sunrise and from sunset on: `night` when the view has it, else `evening`.
   - From sunrise to the middle between sunrise and noon: `morning`.
   - From the middle between noon and sunset to sunset: `evening`.
   - Else: `noon`.
3. The page reads `home-views/manifest.json` and then the image at
   `home-views/<set>/<id>/<slot>.webp`, with no query string. When an image answers 404, the set
   is no longer live: the page reads the manifest again.
