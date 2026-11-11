---
system: device-map-builder
kind: shell
purpose: "Runs the map builder that a rider opens in the browser or on the desktop."
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
status: draft
---

# Device map builder

The body of this document is not written yet.
