---
system: trailview
kind: system
area: Data and services
purpose: "Renders a region as an explorable 3-D landscape in the browser."
owns:
  - host/obc-trailview-features/**
  - builder/web/src/lib/trailview/**
  - tools/trailview/**
  - tools/planner_trailview_canopy.py
depends_on:
  - data-engine
test_depends_on: []
status: draft
---

# TrailView

The body of this document is not written yet. Only the still function is described.

## The still function

A page that does not show the 3-D view can still use one image of it: the cover of a route or a ride, or a home view. `still()` in [`still.ts`](../../builder/web/src/lib/trailview/still.ts) starts an engine of its own on a canvas that the page does not show. It waits until the view is complete, takes the image, and ends the engine. The image is WebP, or JPEG in a browser that cannot encode WebP. The still also gives the credits of the release and of the layers that the engine opened for the view, so that a page shows them with the image.

The script `home-views.mjs` in `builder/web/tests/browser/trailview/` renders the home views with the still function on a GPU of the owner, and uploads them as one set ([specs/home-views.md](../../specs/home-views.md)).

For a line that the caller gives, the still also gives the place of each point in the image, its distance from the eye, and whether it is visible. The engine puts each point on its drawn terrain. A point is hidden only when the drawn terrain rises above the line of sight from the eye to the point: the engine reads the terrain height on that line, in steps that grow with the distance from the eye. Trees, houses, bridges and water do not hide a point. So a page draws the line on the image with no depth image and no camera values.

`coverView()` in [`still-view.ts`](../../builder/web/src/lib/trailview/still-view.ts) chooses the view of a cover before the render, from at most four terrain tiles of the release. For each heading that does not look toward the sun, it finds the lowest tilt that shows 85 % of the route. Of these views, it takes the one that draws the route longest on the screen.

- The caller starts no still while another engine draws: each engine holds hundreds of megabytes of GPU memory.
- A still fails when a tile fails to load, so no still has holes.
- A browser with only a software renderer renders no still (`canRenderStills()`): one still takes minutes there.
