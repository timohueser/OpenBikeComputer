---
system: device-map-builder
kind: shell
purpose: "Runs the map builder, where a rider builds a device map and manages the device, in the browser or as a desktop app."
owns:
  - builder/web/vite.config.ts
  - builder/web/svelte.config.js
  - builder/web/src/main.ts
  - builder/web/src/App.svelte
  - builder/web/src/components/ConfirmDialog.svelte
  - builder/web/src/components/DeviceChip.svelte
  - builder/web/src/components/Gated.svelte
  - builder/web/src/components/Header.svelte
  - builder/web/src/components/UpdatePrompt.svelte
  - builder/web/src/components/device/**
  - builder/web/src/lib/api/**
  - builder/web/src/lib/constants.ts
  - builder/web/src/lib/core/**
  - builder/web/src/lib/desktop/**
  - builder/web/src/lib/download.ts
  - builder/web/src/lib/format.ts
  - builder/web/src/lib/palette.ts
  - builder/web/src/lib/platform/**
  - builder/web/src/lib/router.svelte.ts
  - builder/web/src/lib/routes.ts
  - builder/web/src/lib/theme.svelte.test.ts
  - builder/web/src/lib/theme.svelte.ts
  - builder/web/src/lib/ui/**
  - builder/web/src/routes/**
  - builder/web/tests/**
  - builder/web/test-support/**
  - builder/web/scripts/**
  - builder/web/vite/**
  - builder/desktop/**
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

In the map builder, a rider selects an area, builds a map for the device and sends it to the card. The rider can also see what is on the card, keep the recorded rides, and install firmware updates.

One Svelte frontend runs in three hosts: the public web site, a desktop app made with Tauri ([`builder/desktop`](../../builder/desktop/README.md)), and a development server for maintainers. The desktop app adds native downloads, map files on disk, USB and a ride library. The entry point [`main.ts`](../../builder/web/src/main.ts) loads the shared WebAssembly core, then shows [`App.svelte`](../../builder/web/src/App.svelte) and its pages.

The shell puts together map assembly, map style, route planning, TrailView, the device link and firmware update. It owns the pages, the host layer [`platform/`](../../builder/web/src/lib/platform/index.ts) and the build configuration [`vite.config.ts`](../../builder/web/vite.config.ts). Map assembly owns the WebAssembly core.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The web site's bundle contains no map-build code and no style editor. | `keeps build and style-editor code out of the web target` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts) |
| Each host capability has a reason and a next step that the rider sees. | `has a reason for every capability flag` in [`gating.test.ts`](../../builder/web/src/lib/platform/gating.test.ts) |
| The built app refers to its files with relative paths, so it works under any path of a server. | `references its own assets relatively` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts); the step "Mountable at any prefix" in [`deploy-site.yml`](../../.github/workflows/deploy-site.yml) |

## Decisions

- **The build selects the host.** An alias (`$host`) in [`vite.config.ts`](../../builder/web/vite.config.ts) points at the module of one host, so the code of the other two hosts is not in the bundle. We rejected a test at run time, because then the bundle depends on the bundler to remove code that cannot run.
- **A missing feature stays on the screen.** When a host does not have a feature, its control is disabled and shows a reason and a next step. The rider then reads the explanation when they want the feature. Components state the requirement that they need and do not read the host flags ([`gating.ts`](../../builder/web/src/lib/platform/gating.ts)).
