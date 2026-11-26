# Builder

The web app of the rider and what it needs in the browser: the Svelte app in `web/`, three
Rust-to-WebAssembly bridges, the native USB transport, the map presets and OSM test fixtures.

`web/` builds three pages. The planner is the app at `/plan/` of the site and the only page
that is deployed. The operator page of `obc data ui` runs on the operator's machine. The
TrailView page is a development tool; a rider opens TrailView only from the planner.

Not here: the planner services and search (`planner/`), the data engine (`host/obc-data`), the
Rust cores behind the bridges (`host/obcm-assemble`, `host/obc-ground-plan`,
`host/obc-trailview-parts`), and the device protocol (`firmware/obc-link`).

## Where to start

| Path | What |
| --- | --- |
| [`web/`](web/README.md) | The app. `vite.config.ts` has one build mode for each page. |
| `web/src/planner.ts`, `web/src/Shell.svelte` | The app at `/plan/`: the top bar, one page of `src/pages/` (the table `PAGES`), the footer. The hash selects the page (`lib/routes.ts`). The shell also syncs routes and rides with the device on USB and makes cover pictures in the background. |
| [`web/src/components/planner/`](web/src/components/planner/README.md), `web/src/lib/planner/` | The route planner and its search. |
| [`web/src/components/data/`](web/src/components/data/README.md), `web/src/lib/data/` | The operator page of `obc data ui`. |
| [`web/src/lib/trailview/`](web/src/lib/trailview/README.md) | TrailView: WebGPU and WGSL, the still function, the route lines. |
| `web/src/lib/` | Shared: `core/` (loads the WebAssembly core), `ui/`, `download.ts`, `routes.ts`. Device: `usb/`, `device/`, `device-sync/`, `firmware/`. Maps: `catalog/`, `cells/`, `assemble/`, `coverage/`, `map/`, `convert/`. Style: `skin/`, `color/`. Rider data: `library/`, `covers/`, `settings/`. |
| [`wasm/`](wasm/README.md) (`obc-builder-bridge`) | The core in the browser: conversion, assembly, preview, the device client. |
| [`ground-plan-wasm/`](ground-plan-wasm/README.md), [`trailview-parts-wasm/`](trailview-parts-wasm/README.md) | The TrailView bridges. |
| [`usb/`](usb/README.md) (`obc-usb`) | The native USB transport and `obc format-card`. |
| `web/tests/browser/` | Playwright suites ([planner](web/tests/browser/planner/README.md), data, [trailview](web/tests/browser/trailview/README.md)), the [UX study](web/tests/browser/ux-study/README.md), and the scripts that render home views and route pictures. |
| [`web/tests/device-smoke/`](web/tests/device-smoke/README.md) | The upload smoke test on a real board. |
| `presets/` | The default pack config of `obc pack`, and the skins. |
| [`tests/corpus/`](tests/corpus/README.md) | OSM fixtures of the `host/obc-bake` ingest tests. Not for the web app. |

Before UI work, the `ui` skill (`.agents/skills/ui/SKILL.md`) routes to the design and motion
skills in `.agents/ui/`.

## Run and test

```sh
cd builder/web && npm ci && npm run build:wasm    # needs wasm-pack and the wasm32 target
npm run check && npm test                         # svelte-check, then Vitest
obc site                                          # build the planner as the deploy does; serve on :4173
npm run test:planner --prefix builder/web/tests/browser   # also test:data, test:trailview
obc test -p obc-builder-bridge                    # also obc-usb, obc-ground-plan-bridge
python3 builder/wasm/wasm_size_guard.py           # the size budget of the bridges
obc ux                                            # a UX study; a person starts it, it costs money
```

`obc suites prepare` installs the npm projects and builds the bridges of the web suites. The
suites are `web.builder-vitest`, `web.planner-requests`, `web.data-page`, `web.trailview-smoke`
and `web.ux-study`; `web.trailview-gpu` is manual and needs a GPU.

## Gotchas

- The `pkg/` outputs of the bridges are generated and ignored. Vitest and `svelte-check` need
  them. `obc site` builds them every time, because an existing output does not prove that it
  matches its Rust source.
- The simulated device (`web/test-support/flat-device/`, `lib/usb/flat-device.ts`,
  `loopback.ts`) must never ship. `lib/platform/bundle.test.ts` holds this. Nothing under
  `web/test-support/` may be imported by a shipped build.
- Every asset URL stays relative (`base: "./"`), so the app works under any path of a server.
  `bundle.test.ts` and the deploy step "Mountable at any prefix" hold this.
- `vite build` with an unknown `--mode` stops on purpose. So does a planner build with a
  `VITE_PLANNER_CONFIG` that the planner cannot parse.
- The version stamp of the parts bridge is a hash of its tracked sources. After a change of
  `host/obc-trailview-parts`, the bridge or `data/trailview/parts.json`, build the bridges
  again, or the app refuses the package.
- `web/vite/third-party-licenses.ts` imports no local module: the native search build of
  `planner/search` loads it in plain Node.
- `npm run test:trailview` builds the TrailView page against a fixture. Build for production
  again before you render home views or route pictures.
- Headless Chromium on Linux without a GPU needs SwiftShader, and its times are not GPU times.
  A test that draws for minutes gets the tag `@gpu`.
- The checks of the UX study read the record shape of the library (`lib/library/records.ts`)
  and the points of a plan (`lib/planner/editor.ts`). No test of the study imports them. A
  change of the shape shows only as studies that never end as done.
- `obc data ui` refuses an `Origin` that is not its own host, so the dev proxy keeps the Host
  header.
- The Linux udev rule of [`usb/README.md`](usb/README.md) is needed for WebUSB too.
- After a production data update, run the route pictures steps of
  [`web/tests/browser/trailview/README.md`](web/tests/browser/trailview/README.md).

## Decisions

- One WebAssembly core for each browser realm, loaded once before the synchronous data APIs.
  A worker loads its own.
- The shared web code (`lib/core`, `lib/ui`, `download.ts`, the theme, `routes.ts`) has no
  product rule and imports no feature directory, so each feature can use it with no cycle.
  No check holds this.
- `download.ts` refuses bytes whose size or SHA-256 differs from the published values.
- `router.from` names the page before the current one in this tab. It changes for another page
  or id, never for the settings window, and is undefined after a load. Route planning uses the
  router too, so it is not in the shell.
- Each action of the operator page is one `obc data --json` command, so the page can do only
  what the command line does, and a credential never reaches an argument or a log.
- The output of the test device lies outside `src/`, because nothing shipped may import it. It
  has no size budget: only tests download it.
- The assembly worker is an ES module, because it imports the bridge dynamically and only ES
  output splits code. A browser without module workers loses only the assembly in the browser.
- `svelte-check` runs before each `obc site` build, because Vite strips types without a check.
