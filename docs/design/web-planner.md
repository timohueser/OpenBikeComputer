---
system: web-planner
kind: shell
purpose: "Opens the route planner as its own page in the browser."
owns:
  - builder/web/src/planner.ts
  - builder/web/src/lib/covers/**
depends_on:
  - device-map-builder
  - library
  - route-planning
  - trailview
test_depends_on: []
status: approved
---

# Web planner

## What it does

This page shows the route planner alone in a browser window. The entry point [`planner.ts`](../../builder/web/src/planner.ts) loads the shared WebAssembly core of the map builder, then shows the planner app of route planning (`PlannerApp`). When the core or the planner catalogue cannot load, the page shows a short message. The entry point makes the one library of the page (`Library`) and gives it to the planner, because a listener of the library hears only the writes of its own object.

`npm run build:planner` builds the page into `builder/web/dist/planner`. The site deploy copies it to the path `/plan/` of the project site ([`deploy-site.yml`](../../.github/workflows/deploy-site.yml)). Route planning owns all the planner code. This shell owns the entry point and the covers in `lib/covers`.

## Covers

A cover is one TrailView still image of a route or a ride, 1920 × 1280 pixels, with the place of each point of the summary line in the image. The library keeps it in its store `covers`. The page draws the line from that list of points, so one cover serves each size of box.

The cover job ([`job.ts`](../../builder/web/src/lib/covers/job.ts)) starts after the planner shows. It listens to the library. When the line of the planner's route changes, the job asks for its cover 30 s after the last change, so a rider who edits a route gets one cover at the end. A page asks for a cover with `wantCover`. The job renders one cover at a time in the whole browser: it holds the Web Lock `obc-cover` for each cover, and reads the record again inside the lock. It renders only while the page is visible and online, and not while the planner shows TrailView. When the planner opens TrailView, the job stops the render and does it again later. A record gets the state `failed` after three tries. A still that reaches its time limit stops the job until the page loads again. The job does not run when the browser asks to save data, or when it has no WebGPU or only a software renderer. Then each page shows the fallback.

The cover part ([`Cover.svelte`](../../builder/web/src/lib/covers/Cover.svelte)) cuts the image as CSS `cover` does, and moves the cut toward the middle of the route. It draws the line thinner far from the eye, and a hidden part of the line weak. It shows a selected part and a mark, and gives the point of the line under the pointer. With no cover, it draws the summary line on a plain ground. `planner.html?cover=ID` shows the cover of one record at two sizes, for the browser test.
