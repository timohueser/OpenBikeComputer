# Home views

`home-views.mjs` renders the views of `home-views.json` with the TrailView still function, uploads
them as one set, and makes a set the live set. [specs/home-views.md](../../../../../specs/home-views.md)
has the keys, the manifest and the times of the images. Run the commands from
`builder/web/tests/browser`.

## The list

`home-views.json` is `{date, views}`. A view is `{id, name, region, view, night?}`.

| Field | Value |
| --- | --- |
| `date` | The date of the set: the 15th of the middle month of the season |
| `id` | Lowercase letters, digits and hyphens: the folder of the images |
| `view` | The query string that Ctrl+C copies in the TrailView page. Its date and time are not used. |
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

- `render` skips a view whose images exist and whose `view.json` has the same `view` and `night`.
  A changed view renders all its images again.
- `npm run test:trailview` builds the page against the fixture. Build it again for production
  before you render.
- After a failed tile, `render` waits one minute and tries the image one more time: the tile host
  answers 429 to too many requests. After a second failure it stops; the next run starts at that
  view.
- `upload` and `switch` call `obc data r2`, which needs `OBC_R2_*`. An image key is never replaced,
  so a changed image gives a new set id.
