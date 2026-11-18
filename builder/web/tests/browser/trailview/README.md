# Home views

`home-views.mjs` renders the views of `home-views.json` with the TrailView still function, uploads
them as one set, and makes a set the live set. [specs/home-views.md](../../../../../specs/home-views.md)
has the keys, the manifest and the times of the images. Run the commands from
`builder/web/tests/browser`.

## The list

`home-views.json` is `{date, views}`. A view is `{id, name, region, view, night?}`.

| Field | Value |
| --- | --- |
| `date` | The date of the set. Use the 15th of the middle month of the season. |
| `id` | Lowercase letters, digits and hyphens: the folder of the images |
| `view` | The address that Ctrl+C copies in the TrailView page; the list keeps the part from the `?`. Its date and time are not used. |
| `night` | `true` adds an image at 23:00. Use it only where lights show. |

## Steps

1. Build the page against production: `npm run build:trailview --prefix ../..`. For a local
   release, start `obc data serve local` and set its `VITE_PLANNER_CATALOG` for the build.
2. Open the page, find a viewpoint, press Ctrl+C, and add the address to the list.
3. Look at the previews, 640 × 360, in `.artifacts/home-views/preview/<date>/`:
   `node trailview/home-views.mjs render --preview --gpu`.
4. Render the set, 2560 × 1440, in `.artifacts/home-views/<date>/`:
   `node trailview/home-views.mjs render --gpu`. It prints the seconds of each image.
5. Upload the set: `node trailview/home-views.mjs upload`. It prints the set id.
6. Approve the set id, then make the set live: `node trailview/home-views.mjs switch SET`.
   `curl -sI https://maps.openbikecomputer.com/home-views/manifest.json` answers 200.
   `switch` with the id of the previous set goes back.
7. Delete the set before the previous one:
   `obc data r2 delete --prefix home-views/SET/ --reason "an older home view set"`.

For a new season, change `date`, then do steps 4 to 7.

## Options of render

| Option | Effect |
| --- | --- |
| `--view ID` | Only this view |
| `--out DIR` | Another folder than `.artifacts/home-views` |
| `--list FILE` | Another list than `home-views.json` |
| `--gpu` | The GPU of the machine. Without it, SwiftShader draws for minutes. |
| `--fixture` | The tiles of the test fixture: build the page with `npm run pretest:trailview` |

## Pitfalls

- `render` skips a view whose images exist and whose `view.json` has the same `view`, `night` and
  release. A changed view or a new release renders all its images again. `--fixture` writes the
  release `fixture`; `upload` takes only images of the active production release.
- `npm run test:trailview` builds the page against the fixture. Build it again for production
  before you render.
- After a failed tile, `render` waits one minute and tries the image one more time: the tile host
  answers 429 to too many requests. After a second failure it stops; the next run starts at that
  view.
- `upload` and `switch` call `obc data r2`, which needs `OBC_R2_*`. An image key is never replaced,
  so a changed image gives a new set id. `switch` checks the images in the bucket, because the
  public host keeps a deleted image in its cache for a year.

# Route pictures

`route-pictures.mjs` renders the cover of each route of the route catalog with the TrailView still
function, uploads the covers and the set that names them, and makes a set the live set.
[specs/route-pictures.md](../../../../../specs/route-pictures.md) has the keys, the files and the
key of a cover. Run the commands from `builder/web/tests/browser`.

## Steps

The first runs, from `builder/web/tests/browser`:

1. Build the still page for production: `npm run build:trailview --prefix ../..`. `npm run
   test:trailview` builds it for the fixture, so build it again after each test run. `render`
   refuses a page of another release than production.
2. Run the test of one fixture route on the GPU:
   `npm run test:trailview -- --trace off route-pictures.test.js` with `OBC_TRAILVIEW_GPU=1`. It
   passes with no test skipped. Then build the page again (step 1).
3. Measure a sample: `node trailview/route-pictures.mjs render --gpu --sample 30`. It prints the
   seconds and the bytes of each cover, and the time, the size and the count of files of all routes.
4. Render all routes in `.artifacts/route-pictures/`: `node trailview/route-pictures.mjs render --gpu`.
   It writes `covers/<id>/<key>-0715-1000.webp` and `<key>.json`, or `<key>.none` for a route with no view, and at
   the end the small images `<key>-0715-1000-small.webp` and the set under `sets/<set>/`. Then upload: `node trailview/route-pictures.mjs upload`. It
   prints the set id. Approve the set id, then make the set live:
   `node trailview/route-pictures.mjs switch SET`.
   `curl -sI https://maps.openbikecomputer.com/route-pictures/manifest.json` answers 200.
   `switch` with the id of the previous set goes back.

For a new planner release, do steps 1 and 4: only the routes whose line changed render.

`node trailview/route-pictures.mjs small [--out DIR]` makes the small image of each full image that
has none or an older one, with no render and no network.

## Options of render

| Option | Effect |
| --- | --- |
| `--sample N` | Only N routes, spread over the catalog |
| `--only=ID,ID` | Only these record IDs; a section has a negative ID, which needs the `=` form |
| `--out DIR` | Another folder than `.artifacts/route-pictures` |
| `--gpu` | The GPU of the machine. Without it, SwiftShader draws for minutes. |
| `--fixture` | The routes of `fixture/routes.json` and the tiles of the test fixture, for the test: build the page with `npm run pretest:trailview` |

## Pitfalls

- A route that fails gets no file. `render` goes on with the next route, names each failed route
  with its reason at the end, and exits with 1. The next run tries it again. After 5 failures in a
  row it renders a route with a cover again, and stops if that fails too: the host or the GPU is down.
  When a route fails on each run, leave it out: `upload --without ID,ID`.
- `render` skips a route whose cover or `.none` mark exists for its key. A change of the size, the
  camera rule or its light in the script changes `look` and so each key: all routes render again.
- `upload` takes the routes of the active production release, and refuses while one of them has no
  cover and no `.none` mark, or a cover has no small image. It lists the bucket once and sends only the missing cover files, 16
  at a time, the points first. After a 429 of the host it waits one minute, at most 5 times for a file.
- `upload` and `switch` call `obc data r2`, which needs `OBC_R2_*`. `switch` checks the covers in
  the bucket, because the public host keeps a deleted file in its cache for a year.
