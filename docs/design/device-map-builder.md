---
system: device-map-builder
kind: shell
purpose: "Runs the map builder, where a rider builds a device map and manages the device, in the browser."
owns:
  - builder/web/vite.config.ts
  - builder/web/svelte.config.js
  - builder/web/src/main.ts
  - builder/web/src/App.svelte
  - builder/web/src/components/ConfirmDialog.svelte
  - builder/web/src/components/Header.svelte
  - builder/web/src/components/UpdatePrompt.svelte
  - builder/web/src/components/device/**
  - builder/web/src/lib/api/**
  - builder/web/src/lib/constants.ts
  - builder/web/src/lib/core/**
  - builder/web/src/lib/download.ts
  - builder/web/src/lib/format.ts
  - builder/web/src/lib/palette.ts
  - builder/web/src/lib/platform/**
  - builder/web/src/lib/router.svelte.ts
  - builder/web/src/lib/routes.test.ts
  - builder/web/src/lib/routes.ts
  - builder/web/src/lib/theme.svelte.test.ts
  - builder/web/src/lib/theme.svelte.ts
  - builder/web/src/lib/ui/**
  - builder/web/src/routes/**
  - builder/web/tests/**
  - builder/web/test-support/**
  - builder/web/scripts/**
  - builder/web/vite/**
  - builder/__init__.py
  - builder/build-wasm-bridges.sh
depends_on:
  - device-link
  - firmware-update
  - map-assembly
  - map-style
  - route-planning
  - trailview
  - website
test_depends_on:
  - repository-tooling
status: approved
---

# Device map builder

## What it does

In the map builder, a rider selects an area, builds a map for the device, and downloads it or sends it to the device over the USB cable. The rider can also send routes, export recorded rides as GPX, format the card and install firmware updates.

One Svelte frontend runs in two hosts: the public web site and a development server for maintainers. The entry point [`main.ts`](../../builder/web/src/main.ts) loads the shared WebAssembly core, then shows [`App.svelte`](../../builder/web/src/App.svelte) and its pages.

The shell puts together map assembly, map style, route planning, TrailView, the device link and firmware update. It owns the pages, the host layer [`platform/`](../../builder/web/src/lib/platform/index.ts) and the build configuration [`vite.config.ts`](../../builder/web/vite.config.ts). Map assembly owns the WebAssembly core.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The web site's bundle contains no server client and no style editor. | `keeps build and style-editor code out of the web target` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts) |
| The built app refers to its files with relative paths, so it works under any path of a server. | `references its own assets relatively` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts); the step `Mountable at any prefix` in [`deploy-site.yml`](../../.github/workflows/deploy-site.yml) |

## Decisions

- **The build selects the host.** An alias (`$host`) in [`vite.config.ts`](../../builder/web/vite.config.ts) points at the module of one host, so the code of the other host is not in the bundle. We rejected a test at run time, because then the bundle depends on the bundler to remove code that cannot run.
- **A browser with no WebUSB gets one sentence.** The device session has the state `unsupported` for such a browser, and the device step shows the sentence of the session in place of the Connect button ([`webusb.ts`](../../builder/web/src/lib/usb/webusb.ts)). The download of the map works in each browser.
