---
system: device-map-builder
kind: shell
area: Shells
purpose: "Runs the map builder that a rider opens in the browser or on the desktop."
owns:
  - builder/web/*
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
  - builder/*.py
  - builder/*.sh
depends_on:
  - device-link
test_depends_on: []
status: draft
---

# Device map builder

Phase 2 of the cairn plan writes the body of this document.
