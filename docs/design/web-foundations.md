---
system: web-foundations
kind: system
area: Apps
purpose: "Holds the small parts that the web systems share: the loader of the WebAssembly core, the GPX and FIT files, the theme, the page addresses, the verified download, the colour tokens and the licence notices of the build."
owns:
  - builder/web/src/lib/core/**
  - builder/web/src/lib/format.ts
  - builder/web/src/lib/download.ts
  - builder/web/src/lib/constants.ts
  - builder/web/src/lib/theme.svelte.ts
  - builder/web/src/lib/theme.svelte.test.ts
  - builder/web/src/lib/routes.ts
  - builder/web/src/lib/routes.test.ts
  - builder/web/src/lib/router.svelte.ts
  - builder/web/vite/third-party-licenses.ts
  - builder/web/vite/licenses/**
  - builder/web/src/styles/tokens.css
  - builder/web/test-support/bytes.ts
  - builder/web/test-support/core.ts
depends_on: []
test_depends_on: []
status: draft
---

# Web foundations

## What it does

This system holds the code that more than one web system uses and that has no product rule of its own. The web app, route planning, map assembly, map style, the device link, TrailView, search and the operator page of `obc data` use it.

- [`core/bridge.ts`](../../builder/web/src/lib/core/bridge.ts) loads the shared WebAssembly core one time for the page. The other files of `core/` call it to read and write GPX and FIT files, to calculate SHA-256 and to name the map cells of the grid. Map assembly owns the Rust code of the core.
- [`download.ts`](../../builder/web/src/lib/download.ts) downloads one catalogue object and refuses bytes whose size or SHA-256 differs from the published values.
- [`theme.svelte.ts`](../../builder/web/src/lib/theme.svelte.ts) keeps the look (system, light or dark) of each page.
- [`routes.ts`](../../builder/web/src/lib/routes.ts) gives the hash address of each page of the web app, and [`router.svelte.ts`](../../builder/web/src/lib/router.svelte.ts) follows the address. Route planning uses them too, so they are not in the shell.
- [`tokens.css`](../../builder/web/src/styles/tokens.css) adds the colours of the app and the operator page to the theme of the site.
- [`vite/third-party-licenses.ts`](../../builder/web/vite/third-party-licenses.ts) writes the licence notices of a build. It imports no local file, because the native build of search loads it in plain Node.

## Rules that must hold

| Rule | Check |
| --- | --- |
| This system uses no other system, so each web system can use it with no cycle. | The dependency rule of `cairn check`: `analyse` in [`cairn.py`](../../tools/cairn/cairn.py) reports an import that `depends_on: []` does not list |
