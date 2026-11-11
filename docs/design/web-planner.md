---
system: web-planner
kind: shell
purpose: "Opens the route planner as its own page in the browser."
owns:
  - builder/web/src/planner.ts
depends_on:
  - device-map-builder
  - route-planning
test_depends_on: []
status: approved
---

# Web planner

## What it does

This page shows the route planner alone in a browser window. The entry point [`planner.ts`](../../builder/web/src/planner.ts) loads the shared WebAssembly core of the map builder, then shows the planner app of route planning (`PlannerApp`). When the core or the planner catalogue cannot load, the page shows a short message.

`npm run build:planner` builds the page into `builder/web/dist/planner`. The site deploy copies it to the path `/plan/` of the project site ([`deploy-site.yml`](../../.github/workflows/deploy-site.yml)). Route planning owns all the planner code; this shell owns only the entry point.
