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

## Steps of the first run

1. Build the still page for production: `npm run build:trailview --prefix ../..`. `npm run
   test:trailview` builds it for the fixture, so build it again after each test run. `render`
   refuses a page of another release than production.
2. Run the test of one fixture route on the GPU:
   `npm run test:trailview -- --trace off route-pictures.test.js` with `OBC_TRAILVIEW_GPU=1`. It
   passes with no test skipped. Then build the page again (step 1).
3. Measure a sample: `node trailview/route-pictures.mjs render --gpu --sample 30`. It prints the
   time and the size of each cover and of all routes.
4. Render all routes in `.artifacts/route-pictures/`: `node trailview/route-pictures.mjs render --gpu`.
   It writes the covers, their row images and the set. Then upload:
   `node trailview/route-pictures.mjs upload`. It prints the set id. Approve the set id, then make the set live:
   `node trailview/route-pictures.mjs switch SET`.
   `curl -sI https://maps.openbikecomputer.com/route-pictures/manifest.json` answers 200.
   `switch` with the id of the previous set goes back.

## After a production data update

```sh
# One minute after the update, as the public host keeps the catalog for up to 60 s. The first line
# must name the new release. Keep the live set id that it prints, for a rollback. --without takes the
# list of the last run, from its pull request or issue.
node trailview/route-pictures.mjs check --without=ID,ID
# When check exits with 1: build the page (step 1), then:
node trailview/route-pictures.mjs render --gpu --out DIR
# --without=ID for each route that fails on each run, has no view or has a faulty cover (a cover
# that the owner rejects); the pull request or the issue gives the reason. upload prints the set id.
node trailview/route-pictures.mjs upload --out DIR --without=ID,ID
# Show the new covers to the owner. Switch only a set id that the owner approved.
node trailview/route-pictures.mjs switch SET
# One minute after switch, as the public host keeps the manifest for up to 60 s. Then check prints
# the new set id, and marks the routes of --without as left out.
node trailview/route-pictures.mjs check --without=ID,ID
# No production data update between render and switch. After a data rollback, switch back to the
# live set id that the first check printed.
```

Before a run, the output folder holds the points file, the full image and the row image of each
route that keeps its cover. A folder without them renders those routes again with the camera from
the bucket and sends nothing new. Copy the folder with `cp -a`, so that each row image stays newer
than its full image.

## Other commands

| Command | Effect |
| --- | --- |
| `row [--out DIR]` | Makes each missing or older row image, with no render or network. A broken image gives exit code 1. |
| `check [--without=ID,ID]` | Prints the production release, the live set id and each route of the release with no cover in the live set, from public files only. A route of `--without` is printed and counted, with the mark `(left out)`. Exit code 1 when a route with no cover is not in `--without`. |

## Options of render

| Option | Effect |
| --- | --- |
| `--sample N` | Only N routes, spread over the catalog |
| `--only=ID,ID` | Only these record IDs. A list that starts with a negative ID needs the `=` form: `--only=-ID`, `--without=-ID`. |
| `--out DIR` | Another folder than `.artifacts/route-pictures` |
| `--gpu` | The GPU of the machine. Without it, SwiftShader draws for minutes. |
| `--fixture` | The routes of `fixture/routes.json` and the tiles of the test fixture, for the test: build the page with `npm run pretest:trailview` |

## Pitfalls

- A route that fails gets no file. `render` goes on with the next route, names each failed route
  with its reason at the end, and exits with 1. The next run tries it again. After 5 failures in a
  row it renders a route with a cover again, and stops if that fails too: the host or the GPU is down.
- `render` skips a route whose cover or `.none` mark exists for its key. A change of the size, the
  camera rule or its light in the script changes `look` and so each key: all routes render again.
- `upload` takes the routes of the active production release, and refuses while one of them has no
  cover and no `.none` mark, or a cover has no row image. It sends only the files that are not in
  the bucket. After a 429 of the host it waits one minute, at most 5 times for a file.
- `upload` and `switch` call `obc data r2`, which needs `OBC_R2_*`. `switch` checks the covers in
  the bucket, because the public host keeps a deleted file in its cache for a year.

# Phone measurement

| Step | From `builder/web` |
| --- | --- |
| What | `phone.mjs` measures the page on a phone over the local Wi-Fi. One HTTPS origin on the LAN address serves the page, the data of a local release, `phone-measure.js` and a collector. The phone needs only the Wi-Fi. |
| 1. Data | `obc data serve local` (port 8710), or another origin with `--data` |
| 2. Build | `node tests/browser/trailview/phone.mjs build --origin https://IP:8770` |
| 3. Timing cells (optional) | The `requests` command of the ground plan debug tool (`host/obc-ground-plan/README.md`); give its folder with `--requests` |
| 4. Serve | `node tests/browser/trailview/phone.mjs serve --host IP [--requests DIR] [--results DIR]`. The first start makes a self-signed certificate for `IP` in `--cert`. |
| 5. Phone | Open `https://IP:8770/phone/` and tap Start. The session takes about 8 minutes. |
| 6. Result | `node tests/browser/trailview/phone.mjs summarize RESULTS/SESSION.json` |

| Item | Value |
| --- | --- |
| Runs | Each scene of `phone-measure.js` with the variants `today`, `new` (`ground=plan`) and `new-low` (`ground=plan&scale=0.5&pool=0.5`). Each run is a new page load. At the end, the timing page builds the `--requests` cells. |
| Cache | Every answer is `no-store`, and the script deletes the Cache Storage before each run: each run loads cold. |
| First proper render | The first frame after `nearReady()` in which no road mesh, feature tile, record block or piece mesh is on its way (`meshesLoading` false) |
| Flight | 20 s. The pose follows the clock, so a slow device flies the same path. |
| Frame interval | The time between the animation frames in which the engine draws. Over 17.5 ms and 34.2 ms: one and two display periods of 60 Hz, with 0.8 ms of clock jitter. |
| GPU time | The pass times when the device has `timestamp-query`; on every 10th frame the time from the last submit to `onSubmittedWorkDone` |
| Results | `--results/SESSION.json`. The script posts after each stage, so a killed tab leaves its data. A run whose tab the system reloads is recorded as such. |
| Secure context | WebGPU needs one. On iOS, accept the certificate warning, or install `https://IP:8770/phone/cert.cer` as a profile and trust it under Certificate Trust Settings. |
| Network | The server binds to the LAN address only. A router with client isolation blocks the phone. |
| Fault check | `https://IP:8770/phone/diagnose`, about 5 minutes: `phone-diagnose.js` flies the walk flight with today's app, with a picture every 2 s, then loads `ground=plan`. It posts the WebGPU errors per shader module, pipeline and allocation, the console warnings and the worker errors, and the drawn pages after the flight. For each refused pipeline it stubs functions of the shader to find the one that the compiler rejects. Pictures and bytes go to `RESULTS/SESSION/`. |
| Cold start check | `https://IP:8770/phone/cold`, about 2 minutes: a cold load of the walk view, then a second one with `debug=normals` (`?views=` names others). A picture each second from the first drawn frame through the walk flight, with the loads, stamps and ground jobs in flight; the drawn pages when the load is ready and after the flight; the ground jobs split by `phone-worker.js` into wait, decode, page build and way back. After the first load, `phone-decode.js` decodes the terrain tiles of zoom 12 and 13 under the eye three ways (2D canvas, WebGL, the WASM of `obc-webp-bridge`) and posts their alphas and their difference to the WASM decode. |
