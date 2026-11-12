---
system: web-planner
kind: shell
purpose: "Opens the app at /plan/: the top bar, the pages, and the route planner as one page."
owns:
  - builder/web/src/planner.ts
  - builder/web/src/Shell.svelte
  - builder/web/src/components/TopBar.svelte
  - builder/web/src/styles/shell.css
  - builder/web/src/pages/**
  - builder/web/src/components/route/**
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

This page is the app of the rider at the path `/plan/`. The entry point [`planner.ts`](../../builder/web/src/planner.ts) mounts the shell ([`Shell.svelte`](../../builder/web/src/Shell.svelte)). The shell shows the top bar ([`TopBar.svelte`](../../builder/web/src/components/TopBar.svelte)), one page, and a footer line with the links of the project site and the legal pages.

The hash of the address selects the page, for example `#/planner` or `#/route/ID` ([`routes.ts`](../../builder/web/src/lib/routes.ts)). The table `PAGES` in the shell has one line for each page. A line gives the label of the link in the top bar, if the page has one, and a function that loads the page. A hash with no line in `PAGES` shows the route planner. Each page goes in `src/pages/`. The route planner is the page of route planning (`PlannerApp`). Its line loads the shared WebAssembly core of the map builder first. When a page cannot load, the shell shows its message.

The planner page stays mounted when another page shows, so its Undo history, its map view and its calculated line stay. The shell hides it and gives it `active` false, and then the planner reads no keys. When the route changes, the shell closes each open modal window, because an open modal window makes the rest of the page inert.

The shell makes the one library of the app (`Library`) and gives it to each page, because a listener of the library hears only the writes of its own object. After the first page shows, the shell starts the cover job.

The Settings page ([`Settings.svelte`](../../builder/web/src/pages/Settings.svelte)) has no link in the top bar; the gear at its right end opens it. It sets the units, the look, the maximum heart rate and the threshold power, and it saves and loads a backup of the library. "Delete all" empties the library with `clear` and loads the page again, because after `clear` the `Library` object of the tab refuses each write.

`npm run build:planner` builds the page into `builder/web/dist/planner`. The site deploy copies it to the path `/plan/` of the project site ([`deploy-site.yml`](../../.github/workflows/deploy-site.yml)). Route planning owns all the planner code. This shell owns the entry point, the shell, the top bar, the pages, their parts in `components/route` and the covers in `lib/covers`.

## Route page

The page `#/route/ID` ([`Route.svelte`](../../builder/web/src/pages/Route.svelte)) shows one planned route. The numbers come from the stored summary, and the time is the stored time. The page calculates the line again from the plan, with its own cache of legs, for the days, the climbs, the waypoints and the GPX and FIT export ([`Facts.svelte`](../../builder/web/src/components/route/Facts.svelte), `routeFacts`). When the calculated length differs from the stored length by more than 1 %, the card says so; the planner saves the new summary when it opens the plan. With no network, the card shows the routing error, and Export gives only the plan file.

The picture has two places. With a cover, the cover is large and the map ([`RouteMap.svelte`](../../builder/web/src/components/route/RouteMap.svelte)) is small in a corner; a click on the small one swaps them. With no cover, the map is large and no picture shows. The cover record keeps the credits that the still gives; with the cover large, a button on the picture shows them with the credits of the basemap. A cover made before the credits were kept shows the credits of the release. The pointer is a share of the summary line from 0 to 1, so the cover and the map mark the same place.

A rename calls `renameRoute`, which is the only write that changes the name of a stored route. "Send to device" and "Do not send" change the list `waiting` of the library with `changeState`. "Open in the planner" goes to `#/planner/ID`: the planner opens that plan and then replaces the address with `#/planner`, so Back returns to the page.

## Covers

A cover is one TrailView still image of a route or a ride, 1920 × 1280 pixels, with the place of each point of the summary line in the image. The library keeps it in its store `covers`. The page draws the line from that list of points, so one cover serves each size of box.

The cover job ([`job.ts`](../../builder/web/src/lib/covers/job.ts)) starts after the first page shows. It listens to the library. When the line of the planner's route changes, the job asks for its cover 30 s after the last change, so a rider who edits a route gets one cover at the end. A page asks for a cover with `wantCover`. The job renders one cover at a time in the whole browser: it holds the Web Lock `obc-cover` for each cover, and reads the record again inside the lock. It renders only while the page is visible and online, and not while the planner shows TrailView. When the planner opens TrailView, the job stops the render and does it again later. A record gets the state `failed` after three tries. A still that reaches its time limit stops the job until the page loads again. The job does not run when the browser asks to save data, or when it has no WebGPU or only a software renderer. Then each page shows the fallback.

The cover part ([`Cover.svelte`](../../builder/web/src/lib/covers/Cover.svelte)) cuts the image as CSS `cover` does, and moves the cut toward the middle of the route. It draws the line thinner far from the eye, and a hidden part of the line weak. It shows a selected part and a mark, and gives the point of the line under the pointer. With no cover, it draws the summary line on a plain ground. `planner.html?cover=ID` shows the cover of one record at two sizes, for the browser test.

The window "Add files" ([`AddFiles.svelte`](../../builder/web/src/pages/AddFiles.svelte)) adds GPX and FIT files, folders and the ZIP exports of services to the library. It gives the file readers of the shared core to the library (`readFiles`), so that the library depends on no other system. A route from a file becomes a plan of the planner with one drawn leg (`planFromFile`). No page opens the window yet: `planner.html?add-files` shows it, for the browser test.
