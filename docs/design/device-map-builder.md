---
system: device-map-builder
kind: shell
purpose: "Holds the build configuration of the web app and the code that its pages share."
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

This document holds the code that the pages of the web app share: the host layer [`platform/`](../../builder/web/src/lib/platform/index.ts), the build configuration [`vite.config.ts`](../../builder/web/vite.config.ts), the shared parts in `lib/ui` and the loader of the shared WebAssembly core. The web app itself is the web planner. Map assembly owns the WebAssembly core.

## Rules that must hold

| Rule | Check |
| --- | --- |
| The built app refers to its files with relative paths, so it works under any path of a server. | `references its own assets relatively` in [`bundle.test.ts`](../../builder/web/src/lib/platform/bundle.test.ts); the step `Mountable at any prefix` in [`deploy-site.yml`](../../.github/workflows/deploy-site.yml) |

## Decisions

- **A browser with no WebUSB gets one sentence.** The device session has the state `unsupported` for such a browser, and the Device page shows a sentence in place of the Connect button ([`webusb.ts`](../../builder/web/src/lib/usb/webusb.ts)).
