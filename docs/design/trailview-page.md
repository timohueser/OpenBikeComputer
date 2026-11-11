---
system: trailview-page
kind: shell
purpose: "Opens TrailView, the 3-D view of a region, as its own page in the browser."
owns:
  - builder/web/src/trailview.ts
depends_on:
  - route-planning
  - trailview
test_depends_on: []
status: approved
---

# TrailView page

## What it does

This page shows TrailView, the 3-D landscape of a planner release, alone in a browser window. The entry point [`trailview.ts`](../../builder/web/src/trailview.ts) asks the browser for WebGPU, and shows a message when the browser does not have it. It reads the active release from the planner catalogue, or a preview release that the build received. Then it starts the TrailView engine on the page's canvas.

The page keeps the current view in its address, so a rider can copy the address and open the same view again. Ctrl+C copies the address of the view. Ctrl+V moves to a pasted address. The page also shows the data credits of the release.

`npm run build:trailview` builds the page into `builder/web/dist/trailview`. The page is a development tool and is not published: a rider opens TrailView only from the route planner. The TrailView system owns the engine. Route planning owns the code that reads the catalogue.
