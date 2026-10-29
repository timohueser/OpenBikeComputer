---
system: device-map-builder
kind: shell
purpose: "Runs the map builder that a rider opens in the browser or on the desktop."
owns:
  - builder/web/vite.config.ts
  - builder/web/svelte.config.js
  - builder/web/src/main.ts
  - builder/web/src/App.svelte
  - builder/web/src/lib/**
  - builder/web/src/components/**
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
test_depends_on: []
status: draft
---

# Device map builder

The body of this document is not written yet.
