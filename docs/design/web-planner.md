---
system: web-planner
kind: shell
purpose: "Opens the app at /plan/: the top bar, the pages, and the route planner as one page."
owns:
  - builder/web/vite.config.ts
  - builder/web/svelte.config.js
  - builder/build-wasm-bridges.sh
  - builder/web/src/planner.ts
  - builder/web/src/Shell.svelte
  - builder/web/src/components/TopBar.svelte
  - builder/web/src/styles/shell.css
  - builder/web/src/pages/**
  - builder/web/src/components/route/**
  - builder/web/src/lib/covers/**
  - builder/web/src/lib/device-sync/**
  - builder/web/src/lib/platform/**
  - builder/web/src/lib/ride/**
  - builder/web/test-support/device-sync/**
  - builder/web/tests/browser/planner/**
  - builder/web/tests/browser/playwright.planner.config.js
  - builder/web/tests/browser/serve.py
  - builder/web/tests/browser/package*.json
depends_on:
  - device-link
  - firmware-update
  - library
  - map-assembly
  - map-style
  - route-planning
  - trailview
  - web-foundations
test_depends_on:
  - repository-tooling
status: approved
---

# Web planner

## What it does

The web planner is the app of the rider at the path `/plan/` of the project site. The entry point [`planner.ts`](../../builder/web/src/planner.ts) mounts the shell ([`Shell.svelte`](../../builder/web/src/Shell.svelte)): the top bar, one page and a footer. The hash of the address selects the page ([`routes.ts`](../../builder/web/src/lib/routes.ts)). The table `PAGES` in the shell has one line for each page in `src/pages/`: Home, the route planner, Explore (the signed hikes of the route catalog), the list of routes and rides, a page for one route, ride, signed route or long trail, and the device pages. The settings are a window over the current page.

The shell syncs routes and rides with the remembered device on the USB cable ([`state.svelte.ts`](../../builder/web/src/lib/device-sync/state.svelte.ts)). It also makes the cover pictures of routes and rides in the background ([`job.ts`](../../builder/web/src/lib/covers/job.ts)). Route planning owns the planner code. Web foundations owns the code that all web systems share.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The built app refers to its files with relative paths, so it works under any path of a server. | `references its own assets relatively` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts); the step `Mountable at any prefix` in [`deploy-site.yml`](../../.github/workflows/deploy-site.yml) |

