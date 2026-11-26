# Web app

The Svelte pages of the planner, the data page and TrailView. Run the commands in `builder/web`.
`npm run build:wasm` needs `wasm-pack` ([builder core](../wasm/README.md)).

```sh
npm ci
npm run build:wasm
npm run check
npm test
```

`obc site` does these steps, builds the planner and serves it on port 4173.

## Commands

| Command | Result |
| --- | --- |
| `npm run dev` | The development server. A page is at `/<page>.html` |
| `npm run build` | The same as `npm run build:planner` |
| `npm run build:planner` | `dist/planner/` from `planner.html` |
| `npm run build:data` | `dist/data/` from `data.html` |
| `npm run build:trailview` | `dist/trailview/` from `trailview.html` |
| `npm run build:wasm` | The wasm bridges, with `../build-wasm-bridges.sh` |
| `npm run check` | `svelte-check`. A warning is a failure |
| `npm run check:wgsl` | The TrailView WGSL files, with `naga` (`cargo install naga-cli --version 30.0.1 --locked`) |
| `npm test` | Vitest |

`vite --mode data` sends `/api` to `obc data ui` on port 8700.

## Browser tests

Playwright suites are in `tests/browser`. Run them from the repository root:

```sh
npm ci --prefix builder/web/tests/browser
npm run test:planner --prefix builder/web/tests/browser
npm run test:data --prefix builder/web/tests/browser
npm run test:trailview --prefix builder/web/tests/browser
```

The [planner](tests/browser/planner/README.md), [TrailView](tests/browser/trailview/README.md)
and [UX study](tests/browser/ux-study/README.md) READMEs have the details.

## Environment

| Variable | Effect |
| --- | --- |
| `VITE_SITE_BASE` | The base URL of the site. The default is `/` for links and `https://openbikecomputer.com/` for absolute URLs |
| `VITE_PLANNER_CATALOG`, `VITE_PLANNER_CONFIG`, `VITE_CATALOG_URL` | The catalogue and configuration of the planner ([planner README](src/components/planner/README.md#client-configuration)) |
| `VITE_FAKE_DEVICE` | A build with `1` puts a pretend device on the cable for `?fake-device`. The development server always can |
