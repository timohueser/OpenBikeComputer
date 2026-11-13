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
  - builder/web/src/lib/device-sync/**
  - builder/web/src/lib/ride/**
depends_on:
  - device-link
  - device-map-builder
  - firmware-update
  - library
  - map-assembly
  - map-style
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

With a cover, the picture shows the whole cover, never cut, and the map ([`RouteMap.svelte`](../../builder/web/src/components/route/RouteMap.svelte)) as a card beside it at the same height. The cover is as high as the frame, but no higher than leaves a third of the width to the map; the place under a lower picture stays free. When the map has less than 260 px beside the cover, the cover is large and cut, and the map is small in a corner; a click on the small one swaps them. With no cover, the map is large and no picture shows. The cover record keeps the credits that the still gives; with the cover large, a button on the picture shows them with the credits of the basemap. A cover made before the credits were kept shows the credits of the release. The pointer is a share of the summary line from 0 to 1, so the cover and the map mark the same place.

The bottom card ([`Rows.svelte`](../../builder/web/src/components/route/Rows.svelte)) shows the waypoints, the heights, the surface and the places of one category along the route, or along one day of it. The rows share one axis ([`Lanes.svelte`](../../builder/web/src/components/route/Lanes.svelte)): the distance from the start of the route. The heights ([`ElevationRow.svelte`](../../builder/web/src/components/route/ElevationRow.svelte)) have the steepness colours of the planner profile (`gradeBands`), and the climbs are those of `routeFacts`. With no calculated line, the card shows the heights of the summary and the waypoints at the nearest points of the summary line (`routePins`). Places load only when their row is on the screen, for at most 300 km of the shown stretch. A pointer on the card marks the cover and the map, and the reverse. The large picture, the cover or the map, shows each waypoint as its number in a disc and each night as a moon; a click shows the name. The small picture in the corner shows no pins.

The page has one selected part of the route, as shares of the line. `Lanes` owns how a rider selects it, so that each card on one axis selects a part the same way: a drag, Shift with an arrow key, the line of numbers with Clear in the header, Escape, and the paler rest of the rows. A climb fills its stretch under the profile line. A click on a climb in the elevation row, Enter at a climb, or a click on it in the left card selects its stretch as the part, and the left card then opens the climb: the open climb and the part are one state. A chosen day acts as the part while no part is selected. The map fits the part and draws the rest of the line pale; the cover draws the part amber and the rest pale (`REST` of `draw.ts`). A climb under the pointer marks its stretch the same way, but the map does not move. A place icon is a button: a group opens a list of at most 30 names, and a place opens its name and marks it on the map.

A rename calls `renameRoute`, which is the only write that changes the name of a stored route. "Send to device" and "Do not send" change the list `waiting` of the library with `changeState`, and the device sync sends the routes of that list. "Open in the planner" goes to `#/planner/ID`: the planner opens that plan and then replaces the address with `#/planner`, so Back returns to the page. `#/planner/new` opens a new plan in the same way.

The Route page and the Routes page show the same state of a route on the device ([`state.ts`](../../builder/web/src/pages/routes/state.ts)): "Waits for the device" for a route in `waiting`; "On the device" for a route in `sent` on the card of the remembered device; "Changed since it was sent" when its name or its trip differs from the sent ones (`sentKey`). "Send to device" is off only for a route that is on the device.

The Route page and the Ride page share their frame ([`Detail.svelte`](../../builder/web/src/components/route/Detail.svelte)): the title row with rename, the left card, the picture ([`Picture.svelte`](../../builder/web/src/components/route/Picture.svelte)) and the Export menu. The button styles of the frame apply only inside it, so they do not reach the planner.

## Ride page

The page `#/ride/KEY` ([`Ride.svelte`](../../builder/web/src/pages/Ride.svelte)) shows one ride. A ride record holds no samples, so the page reads the stored source file one time for each key ([`samples.ts`](../../builder/web/src/lib/ride/samples.ts)): FIT with `readFit`, GPX with `readGpx`, and a device ride with `deviceSamples`. It reads the record again after each write of the ride, for example a rename. Distance, climb and the moving time come from the summary. The speed is the metres of the moving steps over their seconds, as on the device. Heart rate, power and cadence are means over the moving steps of the summary (`movingSteps`), weighed by their seconds ([`numbers.ts`](../../builder/web/src/lib/ride/numbers.ts)); a cadence of 0 does not count. A sensor with no value shows no row and no zones. The zones use the maximum heart rate and the threshold power that the ride brought, else those of the settings; with neither, the card links to Settings. The climbs use the one climb rule (`routeSegments`) in each segment of the ride by itself, so a pause adds no hop to a climb. Their distance has no hop across a pause. A climb shows no grade: recorded heights make it too high.

The line of a ride is warm white with an amber glow on the cover, and `--ride` on the map and on the plain ground. Export gives the stored file for its own format, else a file written from the samples, and a CSV table in SI units. "Ride it again" opens the route of the ride's line. Its id is the id that "Add files" gives the same line (`lineId`), so a second click opens the same route. After Delete, the page goes to `#/routes/ridden`.

## Routes page

The page `#/routes` ([`Routes.svelte`](../../builder/web/src/pages/Routes.svelte)) lists the planned routes, and `#/routes/ridden` lists the rides. Each view has its own entry of the history and its own search. The page shows them on a map ([`RoutesMap.svelte`](../../builder/web/src/pages/routes/RoutesMap.svelte)), and shows the selected row in a card over the map ([`SelectedCard.svelte`](../../builder/web/src/pages/routes/SelectedCard.svelte)). The list shows from the library alone. The map, the place search and the titles of plans with no name load the map catalogue, so they come after the list.

The list, the map and the card read plain items: kind, id, name, summary and a time, the last change of a route or the start of a ride ([`filter.ts`](../../builder/web/src/pages/routes/filter.ts)). The rides have the chips place, year, kind of bike and length. A route that is not a plan has no summary: it shows no line and no send button. The search is one object of chips (`Chips`): kind of bike, place, length, climb and days. The rider sets them from a menu ([`Chips.svelte`](../../builder/web/src/pages/routes/Chips.svelte)), or presses Enter in the field: `readSearch` turns a place that the text names into a chip and keeps the rest of the text. A later search that understands requests can fill the same chips. The page keeps the chips and the text in `history.state.search`, so Back brings them again, and another page can give them there. `results` keeps the items that match the text of the field and each chip. A place matches a line that comes within 2 to 10 km of the bounds of the place, by its kind; the nearest line comes first, else the last changed. "Only in this map view" keeps the routes with a point in the view, and the map then does not move by itself.

The map draws the lines from zoom 8. Below zoom 8, bubbles count the shown routes, and a click on a bubble zooms in. The selected route draws wide at each zoom. A row of the list shows its picture only while it is near the view, and it asks the cover job for nothing. The card asks for the cover and the map picture, and shows the credits of the image. Its one link, the name of the route, covers the whole card. A click on the map where no route is, or Escape, closes the card.

The ridden view shows each ride in one row ([`rows.ts`](../../builder/web/src/pages/routes/rows.ts)): its trip, else its group when the group has 2 or more shown rides, else its own row. A trip row holds the whole trip when one of its rides is shown. The page computes the groups and the trips each time it reads the library, so a new route, a changed route or a loaded backup needs no repair. `groups` of the library puts the rides of one road in the newest route they follow, else in the group of the first ride they follow, and leaves out the rides in `apart`. It runs after the first draw. `trips` keeps the trip of a device ride; other rides join the trip of the ride before by where and when they start. The footer shows the totals of the year of the year chip, else of this year (`yearTotals`). A planned route counts the rides of its group: "ridden N times".

The map of the rides draws `--ride` lines with a `--panel` casing over a pale map: a layer of `--parchment` at 50 % over the basemap. The selected row draws all its rides wide. The card of a ride shows its date, numbers and profile. The card of a group shows its newest ride, with "Take out of the group", which adds the ride to `apart`. The card of a trip shows its dates, its days and its totals, and its link opens day 1. "Ride it again" opens the route of a group, else the route of the ride's line (`rideAgain` of [`again.ts`](../../builder/web/src/lib/ride/again.ts)).

## Device sync

The shell syncs routes and rides with one device on the cable: the remembered device ([`remembered.svelte.ts`](../../builder/web/src/lib/device-sync/remembered.svelte.ts)). Only the Connect button of the Device page remembers a device (`rememberDevice`). `localStorage` keeps it in the key `obc-device`. With no remembered device, the shell opens no device session, and no sync runs.

A sync ([`state.svelte.ts`](../../builder/web/src/lib/device-sync/state.svelte.ts)) starts on each `ready` of the remembered device, after `rememberDevice`, on `deviceSync.run()`, and when a route starts to wait or to leave. It holds the Web Lock `obc-device-sync`, so one tab syncs at a time. The sync ([`sync.ts`](../../builder/web/src/lib/device-sync/sync.ts)) does these steps in this order:

1. It lists the card. `sent` names each route on a device as `storeId:objectId`, so a route sent to another card is not on this card: the sync removes nothing for it, and a new send creates it.
2. It takes each finished ride that the library does not hold and did not delete, the oldest first, and stores it with its file. The ride brings the maximum heart rate and the threshold power of the device to the settings. A demo ride, a ride with no points and a ride with no clock stay on the device, and `skipped` remembers them.
3. When the browser keeps the library on a full disk (`persist`), the sync marks the stored rides on the device as taken (`ARCHIVE_RIDE`). It marks a ride only when the stored file has the length and the CRC of the object on the card.
4. It removes each route of `leaving` from the device. A device refusal of one ride, one mark or one route goes into `problem`, and the sync goes on.
5. It sends each route of `waiting` as one OBCR route, and keeps in `sent` the `sentKey` of the route: a hash of its name and its plan. A route whose key differs changed since it was sent. A route that the rider deleted during its upload goes into `leaving`. Each route goes as one OBCR route, also a plan of several days. It calculates the line again with the routing service, because the summary line is too coarse for the device. The waypoints are the nights, then the stops, 32 at most. A route keeps its object on the device, so a new send replaces it. With no network, the route waits.
6. It forgets each route of `sent` whose object is no longer on the card, so that the rider can send it again. It remembers the map, the number of routes and the bytes on the card, and the rides that came in today.

A pulled cable stops the sync. The library stores a ride only after its whole download, and a route goes into `sent` only after its upload, so the next connection continues. After "Delete all", the next connection takes each ride again, and the sent routes stay on the device.

## Device page

The page `#/device` ([`Device.svelte`](../../builder/web/src/pages/Device.svelte)) shows the remembered device and the device sync. "Sync" means that routes go to the device and rides come from it; "update" means only a software update. The page does not run a step of the sync itself. It opens the one device session of the tab (`deviceHolder`), and the sync uses the same session, so the tab has one link to the device. The first state that applies shows:

| State | When | The page shows |
| --- | --- | --- |
| Cannot | The browser has no WebUSB. | That the cable works in Chrome and Edge, and what works here. No button. |
| Connected | The remembered device is `ready`. | The name, "Connected", "Disconnect", the live drawing, and one card of rows: Sync, Routes, Rides, Memory card, Map and Software. |
| Not connected | A device is remembered. | The name, "Not connected", "Connect", the dim drawing, the last sync and the routes that wait for the device. |
| Never | No device is remembered. | "Connect a device", and a note that the product works with no device. |

The row Sync shows the time of the last sync (`at` of the remembered device) and what the last sync of this page did (`deviceSync.report`). "Sync now" and, after a problem, "Try again" call `deviceSync.run()`. The row Routes counts the routes on the card and names the routes of the library whose `sent` entry names this card. The row Rides counts the rides of the library that came from this card, from `library.rides()`, because a ride record names its card and the library has no index of it. The memory card bar shows only the used parts, because the cable does not report the size of the card.

The row Software ([`Software.svelte`](../../builder/web/src/pages/device/Software.svelte)) compares the version that the cable reports with the published one (`firmwareCheck`). "Update" sends the package to the card (`stageFirmware`) and asks the device to install it (`armUpdate`). The row says that the device installs it only when the device agreed. The firmware refuses that step today, and the row says so in one sentence.

A Connect button calls `requestDevice()` in the click, and remembers the device when it is `ready`. When another device is `ready` on the cable, the button names it and remembers it with no chooser, because a second chooser would open a second link to it. A device whose card has no format cannot be remembered; the page sends the rider to the map builder. "Disconnect" frees the device for another page; the sync takes it again at its next `ready`. The error of a device that another page holds is one plain sentence.

The drawing ([`Drawing.svelte`](../../builder/web/src/pages/device/Drawing.svelte)) is the device as the iOS app draws it. Its screen shows a sample map in the catalog skins `default` (Day) and `dusk` (Night), with `openLiveSkinPreview` of map style. The preview draws 240 × 240 pixels, so the page adds the lower 80 rows of the 240 × 320 screen from a second frame with the camera moved down.

The top bar shows a chip with the name of the remembered device: a hollow dot, or a green dot while it is connected. While a sync runs, a sync icon turns beside it. After a problem of the last sync, the icon is a button that starts the sync again.

`planner.html?fake-device` puts the wasm test device on a pretend cable ([`fake-device.ts`](../../builder/web/test-support/device-sync/fake-device.ts)), with `window.deviceTest`. The dev server has it, and the build of the browser tests has it through `VITE_FAKE_DEVICE=1`. Another build has no path to it.

## Covers

A cover is one TrailView still image of a route or a ride, 1920 × 1280 pixels, with the place of each point of the summary line in the image. The library keeps it in its store `covers`. The page draws the line from that list of points, so one cover serves each size of box.

The cover job ([`job.ts`](../../builder/web/src/lib/covers/job.ts)) starts after the first page shows. It listens to the library. When the line of the planner's route changes, the job asks for its cover 30 s after the last change, so a rider who edits a route gets one cover at the end. A page asks for a cover with `wantCover`. The job renders one cover at a time in the whole browser: it holds the Web Lock `obc-cover` for each cover, and reads the record again inside the lock. It renders only while the page is visible and online, and not while the planner shows TrailView. When the planner opens TrailView, the job stops the render and does it again later. A record gets the state `failed` after three tries. The job does not run when the browser asks to save data. Covers are off when the browser has no WebGPU, and for the rest of the session when the job finds a software renderer or a still that reached its time limit.

A card whose record gets no cover shows a still picture of the map ([`map-picture.ts`](../../builder/web/src/lib/covers/map-picture.ts)). The job makes it when covers are off, or after the cover of the line ends `none` or `failed`. The picture is the planner map with no text, icons or contour lines, 768 × 512 pixels, in the theme of the page. An offscreen MapLibre map draws it, and the job loads that code only for the first picture, so the shell does not load MapLibre at its start. The record of `covers` keeps the picture in its field `map`: one image for each theme, the place of each point of the summary line, and the credits of the map data. A picture waits as a cover does, but not for TrailView. The job makes no more pictures for a line after `none` (the line is outside the map data) or after three failed tries. A done cover removes the picture.

The cover part ([`Cover.svelte`](../../builder/web/src/lib/covers/Cover.svelte)) cuts the image as CSS `cover` does, and moves the cut toward the middle of the route. It draws the line thinner far from the eye, and draws no part of the line that the terrain hides. It shows a selected part and a mark, and gives the point of the line under the pointer. Its prop `place` tells where it shows: a row of a list asks for nothing, a card asks for the cover and then the map picture, and a page asks for the cover only. With no cover, a card shows the map picture of the theme with the line drawn on it as on a cover. Else the part draws the summary line on a plain ground. `planner.html?cover=ID` shows the cover of one record in a home card and at a large size, for the browser test.

The window "Add files" ([`AddFiles.svelte`](../../builder/web/src/pages/AddFiles.svelte)) adds GPX and FIT files, folders and the ZIP exports of services to the library. It gives the file readers of the shared core to the library (`readFiles`), so that the library depends on no other system. A route from a file becomes a plan of the planner with one drawn leg (`planFromFile`). The Routes page opens the window.
