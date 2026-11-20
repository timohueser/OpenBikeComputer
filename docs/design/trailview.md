---
system: trailview
kind: system
area: Data and services
purpose: "Renders a region as an explorable 3-D landscape in the browser."
owns:
  - host/obc-trailview-features/**
  - host/obc-ground-plan/**
  - builder/ground-plan-wasm/**
  - builder/webp-wasm/**
  - builder/web/src/lib/trailview/**
  - builder/web/src/plan-timing.ts
  - builder/web/plan-timing.html
  - builder/web/scripts/**
  - builder/web/vite/stamp-bundle.ts
  - builder/web/vite/stamp-version.d.ts
  - builder/web/vite/bundle-version.ts
  - builder/web/vite/versions.d.ts
  - builder/web/test-support/trailview/**
  - builder/web/test-support/fake-caches.ts
  - builder/web/tests/browser/trailview/**
  - builder/web/tests/browser/playwright.trailview.config.js
  - tools/trailview/**
  - tools/planner_trailview_canopy.py
depends_on:
  - data-engine
test_depends_on:
  - repository-tooling
  - web-planner
status: draft
---

# TrailView

The body of this document is not written yet. Only the still function and the route lines are described.

## The still function

A page that does not show the 3-D view can still use one image of it: the cover of a route or a ride, or a home view. `still()` in [`still.ts`](../../builder/web/src/lib/trailview/still.ts) starts an engine of its own on a canvas that the page does not show. It waits until the view is complete, takes the image, and ends the engine. The image is WebP, or JPEG in a browser that cannot encode WebP. The still also gives the credits of the release and of the layers that the engine opened for the view, so that a page shows them with the image.

The script `home-views.mjs` in `builder/web/tests/browser/trailview/` renders the home views with the still function on a GPU of the owner, and uploads them as one set ([specs/home-views.md](../../specs/home-views.md)).

The script `route-pictures.mjs` beside it renders the cover of each route of the route catalog in the same way, with `coverView()` and the still function. It writes the image and the place of each point of the route's line, and makes a row image of half the size of each image for the list rows. The key of a cover comes from the line, the size and the camera rule, not from the month of the image, so a new planner release needs a render only of the routes whose line changed ([specs/route-pictures.md](../../specs/route-pictures.md)).

For a line that the caller gives, the still also gives the place of each point in the image, its distance from the eye, and whether it is visible. The engine puts each point on its drawn terrain. A point is hidden only when the drawn terrain rises above the line of sight from the eye to the point: the engine reads the terrain height on that line, in steps that grow with the distance from the eye. Trees, houses, bridges and water do not hide a point. So a page draws the line on the image with no depth image and no camera values.

`coverView()` in [`still-view.ts`](../../builder/web/src/lib/trailview/still-view.ts) chooses the view of a cover before the render, from at most four terrain tiles of the release. For each heading that does not look toward the sun, it finds the lowest tilt that shows 85 % of the points of its lines. Of these views, it takes the one that draws the route longest on the screen.

- The caller starts no still while another engine draws: each engine holds hundreds of megabytes of GPU memory.
- A still fails when a tile fails to load, so no still has holes.
- A browser with only a software renderer renders no still (`canRenderStills()`): one still takes minutes there.

## The route lines

The engine draws routes on the drawn terrain: the one route of a plan in the planner, or several routes at once. `showRoutes()` in [`engine.ts`](../../builder/web/src/lib/trailview/engine.ts) takes a list of routes. A route has a line, a colour for each edge, and can have a height profile. The number of a route is its place in the list, from 1. A route with fewer than two points draws nothing and keeps its number.

All routes are one triangle strip in one buffer ([`route/route.ts`](../../builder/web/src/lib/trailview/route/route.ts)), so the CPU does no work for the line in each frame. Each point of the buffer holds the number of its route. Between two routes, the buffer has two break points with the number 0: a copy of the last point of the one route, and a copy of the first point of the next. The shader ([`route/route.wgsl`](../../builder/web/src/lib/trailview/route/route.wgsl)) gives a break point no width, so no line joins two routes. For the direction of the line at a point, the shader uses a neighbour point only when the neighbour has the same number. Each route has round ends: the buffer repeats the first and the last point of a route, and the shader moves each copy half the line width beyond the end and cuts a half circle from the part between.

`boldRoute()` draws one route bold: 2.5 times as wide, and on top of the others where routes share a path. The engine then draws the strip two times in the same pass. The first time it draws all routes but the bold route, and the second time only the bold route. So the bold route draws one time, and its light does not add up. A new list of routes has no bold route.

`routeAt()` gives the number of the route at a point of the canvas, or none. It reads nothing from the GPU, so it answers at once. Each route with a profile has pick points: each vertex of its line, and each entry of its profile between two vertices, at the height that the profile gives there (`profile_m` of [specs/route-catalog.md](../../specs/route-catalog.md)). The engine projects the pick points to the screen with the camera of the last frame, without the jitter (the small shift of each frame that smooths the edges). Of a segment that passes behind the eye, it keeps the part in front of the eye. [`route/pick.ts`](../../builder/web/src/lib/trailview/route/pick.ts) takes the route that has a segment between two pick points within 12 pixels of the point. Of several routes, the bold route wins, then the route nearer to the eye.

- `routeAt()` does not test whether the terrain hides a pick point. So a route behind a ridge answers when no nearer route is at the point.
- A route with no profile is never found.

The controls ([`frame/controls.ts`](../../builder/web/src/lib/trailview/frame/controls.ts)) tell a click from a look around. A look around starts only after the pointer moves more than 4 pixels from the press. A release of the primary button before that, within 300 ms of the press, is a click. Another button looks around at once and never clicks. The options `onclick` and `onhover` of the start of the engine get each click, and each move with no button at most once in each display frame. The page then asks `routeAt()` for the route there.

`coverView()` also takes several lines. It leaves out a line with a point outside the bounds of the release. Each line gets its share of the 200 points that the search tests, by its length, and at least 3. The length of a view is the sum of the drawn lengths of the lines.
