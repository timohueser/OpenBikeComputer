<!--
  The device page: what is on the card, as a gallery — trips as combined-preview bands, routes and
  rides as track-thumbnail tiles, the drop zone as the grid's ghost tile (#894 epic, gallery
  redesign of 2026-07-29; the wireframe's Option A).

  This route is loaded through a dynamic import (`App.svelte`), which is what lets it reach the
  protocol client and codecs directly: nothing here may leak into the entry chunk, and nothing
  here needs to — the session already exists in `deviceHolder`, opened by the header chip.

  Division of labour with the tiles: the tile components (`TripBand`, `RouteTiles`, `RideTiles`)
  render lists and take callbacks; every operation that touches the cable lives here, funneled
  through `dashboard.enqueue` so the page cannot trip the client's one-transfer rule over itself.
  That includes the thumbnails: `deviceThumbs.fill` walks the lists one small download at a time
  through the same queue, so a tile filling in never races a click.

  **A tile shows what a catalog listing knows, a modal shows what a payload knows.** §3.3's entry is
  id, revision, payload length, payload CRC, kind, flags and a display name — so a route's distance
  and a ride's start time are not on a tile, because putting them there would mean downloading every
  object to draw the page. Opening one downloads it, and the modal has the figures. Nothing is
  drawn as a dash in the meantime.
-->
<script lang="ts">
    import { untrack } from "svelte";
    import FirmwareCard from "../components/device/FirmwareCard.svelte";
    import PreviewModal from "../components/device/PreviewModal.svelte";
    import RideTiles from "../components/device/RideTiles.svelte";
    import RouteDrop from "../components/device/RouteDrop.svelte";
    import RouteTiles from "../components/device/RouteTiles.svelte";
    import TransferBar from "../components/device/TransferBar.svelte";
    import TripBand from "../components/device/TripBand.svelte";
    import TripDropDialog from "../components/device/TripDropDialog.svelte";
    import { routeTrack, routeWaypoints, type RouteWaypoint } from "../lib/convert/bridge";
    import { dashboard, type TripView } from "../lib/device/dashboard.svelte";
    import type { ProfilePoint } from "../lib/device/elevation";
    import type { TrackSegment } from "../lib/device/segments";
    import { previewTrack, pullRide, pullRides, type LibraryView, type RideLibrary } from "../lib/device/library";
    import {
        addStage,
        createTrip,
        moveStage,
        removeStage,
        renameRoute,
        stageId,
        updateTrip,
    } from "../lib/device/manage";
    import { recordedRides, rideAccess, rideDistance, rideDuration, rideScope } from "../lib/device/rides";
    import { decodeRouteHeader, type PreparedRoute } from "../lib/device/route";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { DeviceJob, jobRegistry } from "../lib/device/job.svelte";
    import { deviceThumbs, entryFingerprint, STAGE_COLORS, type Thumb, type ThumbRequest } from "../lib/device/thumbs.svelte";
    import { platform } from "../lib/platform";
    import { planTripDelete, type TripStages } from "../lib/device/tripDelete";
    import { confirmAction, confirmChoice } from "../lib/ui/confirm.svelte";
    import { DeviceError, type FlatStoreClient } from "../lib/usb/client";
    import { decodeRideObject } from "../lib/usb/objects";
    import type { CatalogEntry, ObjectRef } from "../lib/usb/protocol";
    import { sendRoute } from "../lib/device/write";

    const session = $derived(deviceHolder.session);
    const client = $derived(session?.status === "ready" ? session.client : null);
    const scope = $derived(rideScope(session?.info ?? null, session?.store ?? null));

    /** The `(ObjectId, Revision)` pair every request names — the listing's own, never the head, so
     *  a request and the listing it came from cannot disagree about which bytes are meant. */
    const refOf = (entry: CatalogEntry): ObjectRef => ({
        objectId: entry.objectId,
        revision: entry.revision,
    });

    /** The shape `tripDelete.ts` reasons over — full-width ids and a name, no protocol types. */
    const asStages = (trip: TripView): TripStages => ({
        objectId: trip.objectId,
        name: trip.displayName,
        detail: trip.detail,
    });

    // Load once per (serial, era); the store survives tab switches, so coming
    // back renders instantly and a card swap reloads.
    $effect(() => {
        if (client) void dashboard.ensureLoaded(client, scope);
    });

    /** One mutation, queued, with the refresh that makes the page the authority again. */
    async function mutate(op: (client: FlatStoreClient) => Promise<unknown>): Promise<void> {
        const c = client;
        if (!c) return;
        try {
            await dashboard.enqueue(() => op(c));
            await dashboard.refresh(c);
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        }
    }

    const routeName = (route: CatalogEntry) => route.displayName || `Route ${route.objectId}`;
    const tripName = (trip: TripView) => trip.displayName || `Trip ${trip.objectId}`;

    const doRenameRoute = (route: CatalogEntry, name: string) => void mutate((c) => renameRoute(c, route, name));

    const doRenameTrip = (trip: TripView, name: string) =>
        void mutate((c) => updateTrip(c, trip, (t) => ({ ...t, name })));

    /**
     * Put a route in a trip: an existing one, or a new trip built around it.
     *
     * The trip is looked up by id in the list the menu was drawn from, because an edit is a
     * read-modify-write that has to carry the revision it expects (§3.6) — a trip something else
     * replaced in between fails the compare-and-swap and the page re-lists, rather than clobbering.
     */
    const doAddToTrip = (route: CatalogEntry, tripId: bigint | null) =>
        void mutate((c) => {
            if (tripId === null) return createTrip(c, routeName(route), [route.objectId]);
            const trip = dashboard.trips.find((t) => t.objectId === tripId);
            if (!trip) throw new Error("That trip is no longer on the device.");
            // `stageId` validates the route's full-width id before writing the trip payload.
            return updateTrip(c, trip, (t) => addStage(t, stageId(route.objectId)));
        });

    const doMoveStage = (trip: TripView, index: number, delta: number) =>
        void mutate((c) => updateTrip(c, trip, (t) => moveStage(t, index, delta)));

    async function doRemoveStage(trip: TripView, index: number) {
        // Removing the last stage would leave an empty grouping — offer to take
        // the trip with it instead of leaving a husk on the card.
        if ((trip.detail?.stages.length ?? 0) <= 1) {
            const ok = await confirmAction({
                title: `Remove the last route from “${tripName(trip)}”?`,
                body: "An empty trip is nothing, so the trip is deleted with it. The route stays on the device.",
                confirmLabel: "Remove and delete trip",
                destructive: true,
            });
            if (!ok) return;
            await mutate((c) => c.remove(refOf(trip)));
            return;
        }
        await mutate((c) => updateTrip(c, trip, (t) => removeStage(t, index)));
    }

    async function deleteRoute(route: CatalogEntry) {
        if (!client) return;
        const ok = await confirmAction({
            title: `Delete “${routeName(route)}” from the device?`,
            body: "The route is removed from the card. A copy on this computer, if you have one, is not touched.",
            confirmLabel: "Delete route",
            destructive: true,
        });
        if (!ok) return;
        await mutate((c) => c.remove(refOf(route)));
    }

    /**
     * Remove an object, treating the device's "not found" as success: an object already gone
     * **is** the state the remove was asked to produce, so a stale plan (or a repeat click)
     * must not abort a delete sequence or surface a banner about it. Real errors still throw.
     * One transfer — callers run it through `mutate`/`enqueue` like any other cable operation.
     */
    const removeIfPresent = (c: FlatStoreClient, ref: ObjectRef) =>
        c.remove(ref).catch((cause: unknown) => {
            if (cause instanceof DeviceError && cause.code === "not-found") return;
            throw cause;
        });

    /**
     * Delete a trip — and, when the card's state allows it, offer to take its routes with it.
     *
     * The offer is computed up front (`planTripDelete`): a route that is also a stage of another
     * trip is never deleted here, and if any other trip's stage list is unreadable the dialog
     * degrades to the grouping-only delete rather than guessing.
     *
     * The dialog can stay open for any amount of time, and the card can change under it — a
     * paired phone editing trips over BLE, most plainly. So what is actually deleted is decided
     * **after** the confirm, not before it: the lists are re-read, the plan recomputed from the
     * fresh state, and (a) a shrunken deletable set simply proceeds — it is strictly safer than
     * what was shown — while (b) a trip whose stage list grew or became unreadable aborts with a
     * one-line note instead of deleting routes the rider was never shown. The deletions then run
     * sequentially through the page's queue — trip first, then each route — with **one** refresh
     * at the end; a mid-sequence failure surfaces one error after that refresh, and whatever was
     * already deleted stays deleted (the refresh shows exactly that).
     */
    async function deleteTrip(trip: TripView) {
        const c = client;
        if (!c) return;
        const name = tripName(trip);
        const routeIds = () => new Set(dashboard.routes.map((r) => r.objectId));
        const plan = planTripDelete(asStages(trip), dashboard.trips.map(asStages), routeIds());

        if (plan.offer === "trip-only") {
            const body = "Only the grouping is removed — its routes stay on the device as ordinary routes.";
            const ok = await confirmAction({
                title: `Delete the trip “${name}”?`,
                body: plan.reason ? `${body}\n${plan.reason}` : body,
                confirmLabel: "Delete trip",
                destructive: true,
            });
            if (!ok) return;
            await mutate(() => removeIfPresent(c, refOf(trip)));
            return;
        }

        const n = plan.deletable.length;
        const choice = await confirmChoice({
            title: `Delete the trip “${name}”?`,
            body: [
                "Deleting the trip only removes the grouping — its routes stay on the device as ordinary routes.",
                plan.note,
            ]
                .filter((line) => line !== null)
                .join("\n"),
            confirmLabel: `Delete trip and its ${n} route${n === 1 ? "" : "s"}`,
            destructive: true,
            extra: { label: "Delete trip only", destructive: true },
        });
        if (choice === "cancel") return;
        if (choice === "extra") {
            await mutate(() => removeIfPresent(c, refOf(trip)));
            return;
        }
        let failure: unknown = null;
        try {
            // Revalidate against the card as it is NOW, not as it was when the dialog opened.
            await dashboard.refresh(c);
            const fresh = dashboard.trips.find((t) => t.objectId === trip.objectId);
            const known = new Set(trip.detail?.stages ?? []);
            if (!fresh || fresh.detail === null || fresh.detail.stages.some((id) => !known.has(id))) {
                dashboard.error =
                    "The trip changed on the device while the dialog was open — nothing was deleted. Try again.";
                return;
            }
            const freshPlan = planTripDelete(asStages(fresh), dashboard.trips.map(asStages), routeIds());
            const deletable = freshPlan.offer === "both" ? freshPlan.deletable : [];
            // The routes as the refresh just listed them, so each remove carries the revision the
            // plan was computed against.
            const byId = new Map(dashboard.routes.map((route) => [route.objectId, route]));
            // The trip goes first: while it exists, its stages are what makes those routes shared.
            await dashboard.enqueue(() => removeIfPresent(c, refOf(fresh)));
            for (const id of deletable) {
                const route = byId.get(id);
                if (route) await dashboard.enqueue(() => removeIfPresent(c, refOf(route)));
            }
        } catch (cause) {
            failure = cause;
        }
        // One refresh either way — after it, the lists say what actually happened; the error
        // (if any) is set after the refresh, which clears the slot on its way in.
        await dashboard.refresh(c);
        if (failure !== null) dashboard.error = failure instanceof Error ? failure.message : String(failure);
    }

    function retry() {
        dashboard.clearBusy();
        if (client) void dashboard.refresh(client);
    }

    /** RouteDrop's transfers ride the page's queue, so a drop cannot collide with a list read. */
    const serialize: <T>(op: () => Promise<T>) => Promise<T> = (op) => dashboard.enqueue(op);

    // --- several files at once: the trip dialog -----------------------------

    let tripDrop = $state<File[] | null>(null);
    const dropJob = new DeviceJob("routes");

    async function addRoutes(routes: PreparedRoute[], name: string | null) {
        const c = client;
        tripDrop = null;
        if (!c) return;
        await dropJob.run(
            async (ctx) => {
                // Sequential — the wire takes one transfer at a time. Each `PUT` creates an
                // object and answers with the id the commit assigned (§3.6), which is what the
                // trip's stage list is then built from.
                const ids: bigint[] = [];
                for (const route of routes) {
                    const { objectId } = await dashboard.enqueue(() => sendRoute(c, route, ctx));
                    ids.push(objectId);
                }
                if (name !== null) {
                    await dashboard.enqueue(() => createTrip(c, name, ids, ctx.signal));
                }
                return { count: ids.length, name };
            },
            (r) =>
                r.name !== null
                    ? `“${r.name}” is on the device, ${r.count} stages.`
                    : `${r.count} routes are on the device.`,
        );
        await dashboard.refresh(c);
    }

    // --- the library's view of the card's rides ------------------------------
    //
    // Only for the badges, the pulls and the free thumbnails: `platform.rides` exists exactly on
    // the tier that also has the Ride-library page. Loaded once, refreshed after every pull.

    let library = $state<RideLibrary | null>(null);
    let libraryView = $state<LibraryView | null>(null);
    const pullJob = new DeviceJob("rides");

    $effect(() => {
        if (!platform.rides || library) return;
        void platform.rides().then(async (opened) => {
            library = opened;
            libraryView = await opened.view();
        });
    });

    /** Rides the device will actually serve: §3.5 refuses a `GET` of one it is still recording. */
    const pullable = $derived(recordedRides(dashboard.rides));

    /** Ride ids a durable copy of which is in the folder — inverse of the library's own list. */
    const heldHere = $derived.by(() => {
        if (!libraryView || scope.epoch === null) return null;
        return new Set(
            libraryView.rides
                .filter((r) => r.present && r.serial === scope.serial && r.epoch === scope.epoch)
                // The index stores an id as a JSON number; the catalog carries the wire's `u64`.
                .map((r) => BigInt(r.objectId)),
        );
    });

    async function refreshLibrary() {
        if (library) libraryView = await library.view();
    }

    async function pullOne(entry: CatalogEntry) {
        const c = client;
        const lib = library;
        if (!c || !lib) return;
        await pullJob.run(
            (ctx) => dashboard.enqueue(() => pullRide(rideAccess(c), lib, scope, entry, ctx)),
            ({ ride }) => `“${ride.name}” is in the library.`,
        );
        await refreshLibrary();
    }

    async function pullAll() {
        const c = client;
        const lib = library;
        if (!c || !lib) return;
        await pullJob.run(
            (ctx) => dashboard.enqueue(() => pullRides(rideAccess(c), lib, scope, ctx)),
            (report) =>
                report.imported.length === 0 && report.repaired.length === 0
                    ? `Nothing new — all ${report.listed} rides on the device are already in the library.`
                    : `Copied ${report.imported.length + report.repaired.length} to the library.`,
        );
        await refreshLibrary();
    }

    // --- thumbnails: session-only on web, durably cached in the desktop app --------------------

    function thumbRequest(
        c: FlatStoreClient,
        kind: "route" | "ride",
        entry: CatalogEntry,
        held: Thumb | undefined,
    ): ThumbRequest {
        return {
            kind,
            // The thumb store keys on a number; an `ObjectId` is a `u64` allocated from a cursor
            // that starts at 1, so every id a card will hold in this decade is exact as a double.
            id: Number(entry.objectId),
            fingerprint: entryFingerprint(entry),
            // A ride already pulled has its preview track in the library index — the free win:
            // no download, and the tile shows exactly what the Ride-library page shows.
            load: held
                ? async () => held
                : async (signal) => {
                      const bytes = (await c.get(refOf(entry), { signal })).bytes;
                      if (kind === "ride") return previewTrack(decodeRideObject(bytes));
                      const points = await routeTrack(bytes);
                      return points.map((p) => [p.lat, p.lon] as [number, number]);
                  },
        };
    }

    $effect(() => {
        const c = client;
        if (!c) return;
        deviceThumbs.ensureScope(scope);
        const heldTracks = new Map<number, Thumb>();
        if (libraryView && scope.epoch !== null) {
            for (const r of libraryView.rides) {
                if (r.present && r.serial === scope.serial && r.epoch === scope.epoch && r.track.length > 1) {
                    heldTracks.set(r.objectId, r.track);
                }
            }
        }
        const requests: ThumbRequest[] = [
            ...dashboard.routes.map((route) => thumbRequest(c, "route", route, undefined)),
            // A ride still being recorded has no payload to draw yet (§3.5), so it is not asked for.
            ...pullable.map((ride) => thumbRequest(c, "ride", ride, heldTracks.get(Number(ride.objectId)))),
        ];
        const aborter = new AbortController();
        // `untrack`: the fill reads (and writes) the thumb store's reactive map, and this effect
        // must re-run on list changes, not on every thumbnail that lands.
        void untrack(() => deviceThumbs.fill(scope, requests, serialize, aborter.signal));
        // Unmount or disconnect: stop walking, and abort the download in flight.
        return () => aborter.abort();
    });

    let thumbnailNotice = $state<string | null>(null);

    function clearSavedPreviews(): void {
        const removed = deviceThumbs.clearPersistent();
        thumbnailNotice =
            removed === 0
                ? "No saved previews were present."
                : `${removed} saved ${removed === 1 ? "preview" : "previews"} deleted. Current previews remain until the app closes.`;
    }

    // --- previews: download the object, decode it, show it ------------------
    //
    // This is also where a route's or a ride's figures come from. They are payload facts, and the
    // payload is exactly what opening a preview fetches — so the modal is the honest place for
    // them, and the tile behind it is not poorer for lacking them.

    /** Aborts preview-driven fetches when the device (or the page) goes away — the trip preview
     *  walks stage tracks through the queue and must not keep walking a dead link. */
    let pageLifetime = new AbortController();
    $effect(() => {
        void client;
        const lifetime = new AbortController();
        pageLifetime = lifetime;
        return () => lifetime.abort();
    });

    let preview = $state<{
        title: string;
        points: ProfilePoint[];
        /** Set for trips: per-stage tracks in the band's colors; `points` is then empty. */
        segments: TrackSegment[] | null;
        stats: Array<{ label: string; value: string }>;
        /** The route's stored waypoints — the modal's floating card + map diamonds. */
        waypoints: RouteWaypoint[];
        /** Set for routes: the modal's header offers Delete. */
        route: CatalogEntry | null;
    } | null>(null);
    /** The object a preview download is running for, to mark the page busy. */
    let previewing = $state<string | null>(null);

    async function previewRoute(route: CatalogEntry) {
        const c = client;
        if (!c || previewing) return;
        previewing = `route-${route.objectId}`;
        try {
            const obcr = (await dashboard.enqueue(() => c.get(refOf(route)))).bytes;
            // Every figure below comes from the OBCR header (§1) of the bytes just downloaded. The
            // catalog carries none of them, and this is the download that makes them knowable.
            const header = decodeRouteHeader(obcr);
            preview = {
                title: routeName(route),
                points: await routeTrack(obcr),
                segments: null,
                // The same downloaded bytes, decoded a second way — the modal's waypoint card,
                // markers and profile ticks. (The modal adds its own "Waypoints" chip when any.)
                waypoints: await routeWaypoints(obcr),
                stats: [
                    { label: "Distance", value: `${(header.distanceM / 1000).toFixed(1)} km` },
                    { label: "Ascent", value: `${header.ascentM.toLocaleString()} m` },
                    { label: "Descent", value: `${header.descentM.toLocaleString()} m` },
                    { label: "Points", value: header.pointCount.toLocaleString() },
                ],
                route,
            };
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            previewing = null;
        }
    }

    async function previewRide(ride: CatalogEntry) {
        const c = client;
        if (!c || previewing) return;
        previewing = `ride-${ride.objectId}`;
        try {
            const object = decodeRideObject((await dashboard.enqueue(() => c.get(refOf(ride)))).bytes);
            preview = {
                title: object.name || ride.displayName || `Ride ${ride.objectId}`,
                points: object.points.map((p) => ({ lat: p.lat1e7 / 1e7, lon: p.lon1e7 / 1e7, ele: p.eleM })),
                segments: null,
                // The ride object's own summary fields (§7.2) — the device computed these at Finish.
                stats: [
                    { label: "Distance", value: rideDistance(object.distanceM) },
                    { label: "Moving time", value: rideDuration(object.movingTimeS) },
                    { label: "Avg speed", value: `${((object.avgSpeedCms / 100) * 3.6).toFixed(1)} km/h` },
                    { label: "Climb", value: `${object.climbM.toLocaleString()} m` },
                ],
                waypoints: [],
                route: null,
            };
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            previewing = null;
        }
    }

    /**
     * Preview a whole trip: every stage's OBCR downloaded and decoded in full — real tracks with
     * elevation, real waypoint tables — as one segment per stage in the band's colors, so the
     * modal can draw the combined profile and the stage-colored map. Downloads run sequentially
     * through the queue (they are small, a route polyline apiece) under the same `previewing`
     * busy state the single-object previews use; the page-lifetime signal cuts the walk short
     * when the device or the page goes away. Dangling stages are skipped, as the band skips them.
     *
     * The stat chips are sums over the downloaded stages' own headers — the same bytes the
     * segments are drawn from — plus the stage count. They exist here and not on the band for the
     * same reason: a trip's totals are payload arithmetic, and this is where the payloads are.
     */
    async function previewTrip(trip: TripView) {
        const c = client;
        if (!c || previewing) return;
        const stages = dashboard.stagesOf(trip);
        if (!stages.some((s) => s.route !== null)) return;
        previewing = `trip-${trip.objectId}`;
        const signal = pageLifetime.signal;
        try {
            const segments: TrackSegment[] = [];
            let distanceM = 0;
            let ascentM = 0;
            let descentM = 0;
            for (const [index, stage] of stages.entries()) {
                const route = stage.route;
                if (!route) continue; // dangling: skipped, exactly as the band draws it
                const obcr = (await dashboard.enqueue(() => c.get(refOf(route), { signal }))).bytes;
                const header = decodeRouteHeader(obcr);
                distanceM += header.distanceM;
                ascentM += header.ascentM;
                descentM += header.descentM;
                segments.push({
                    name: route.displayName || `Route ${stage.id}`,
                    // By position in the FULL stage list, dangling included — the same cycle the
                    // band's dots use, so a row's dot and its drawn segment always agree.
                    color: STAGE_COLORS[index % STAGE_COLORS.length],
                    points: await routeTrack(obcr),
                    waypoints: await routeWaypoints(obcr),
                });
            }
            // The chip counts the FULL stage list, dangling included — the same count the trip
            // band shows — and says plainly when some of it could not be drawn.
            const missing = stages.length - segments.length;
            preview = {
                title: tripName(trip),
                points: [],
                segments,
                stats: [
                    { label: "Distance", value: `${(distanceM / 1000).toFixed(1)} km` },
                    { label: "Ascent", value: `${ascentM.toLocaleString()} m` },
                    { label: "Descent", value: `${descentM.toLocaleString()} m` },
                    { label: "Stages", value: missing > 0 ? `${stages.length} · ${missing} missing` : `${stages.length}` },
                ],
                waypoints: [],
                route: null,
            };
        } catch (cause) {
            // An abort is the page (or the cable) going away, not something to report.
            if (!signal.aborted) dashboard.error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            previewing = null;
        }
    }
</script>
<article>
    {#if deviceHolder.interrupted}
        <p class="note error small" role="alert">{deviceHolder.interrupted}</p>
    {/if}

    {#if client && session}
        <div class="idrow">
            <h1>OpenBikeComputer</h1>
        </div>

        {#if dashboard.busy}
            <p class="note small" role="status">
                Another transfer is holding the cable
                {#if jobRegistry.active}(sending {jobRegistry.active.label} — see the top bar){/if}
                — the lists will load once it finishes.
                <button type="button" class="btn ghost" disabled={jobRegistry.active !== null} onclick={retry}>
                    Retry
                </button>
            </p>
        {/if}

        {#if dashboard.error}
            <p class="note error small" role="alert">{dashboard.error}</p>
        {/if}

        {#if dashboard.loading && dashboard.routes.length === 0 && dashboard.rides.length === 0}
            <p class="small muted">Reading the card…</p>
        {/if}

        <section>
            <div class="secline">
                <h3>Routes</h3>
                <span class="small faint">
                    {dashboard.routes.length}
                    {dashboard.routes.length === 1 ? "route" : "routes"}{#if dashboard.trips.length}
                        · {dashboard.trips.length}
                        {dashboard.trips.length === 1 ? "trip" : "trips"}{/if}
                </span>
            </div>

            {#each dashboard.trips as trip (trip.objectId)}
                <TripBand
                    {trip}
                    stages={dashboard.stagesOf(trip)}
                    trackFor={(id) => deviceThumbs.get("route", Number(id))}
                    busy={previewing !== null}
                    onopen={() => void previewTrip(trip)}
                    onopenstage={(route) => void previewRoute(route)}
                    onrename={(name) => doRenameTrip(trip, name)}
                    ondelete={() => void deleteTrip(trip)}
                    onmovestage={(index, delta) => doMoveStage(trip, index, delta)}
                    onremovestage={(index) => void doRemoveStage(trip, index)}
                />
            {/each}

            <RouteTiles
                routes={dashboard.topLevelRoutes}
                trips={dashboard.trips}
                trackFor={(id) => deviceThumbs.get("route", Number(id))}
                busy={previewing !== null}
                onopen={(route) => void previewRoute(route)}
                onrename={doRenameRoute}
                ondelete={(route) => void deleteRoute(route)}
                onaddtotrip={doAddToTrip}
            >
                <RouteDrop
                    {client}
                    {serialize}
                    heading={null}
                    empty={dashboard.topLevelRoutes.length === 0 && dashboard.trips.length === 0}
                    onmultiple={(files) => (tripDrop = files)}
                    onsent={() => client && void dashboard.refresh(client)}
                />
            </RouteTiles>
            <TransferBar job={dropJob} />
        </section>

        <!-- The routes block above, the rides ledger below: a wider gap and a hairline rule, so
             the two galleries read as two sections rather than one long grid. -->
        <section class="rides">
            <div class="secline">
                <h3>Rides</h3>
                <span class="small faint">{dashboard.rides.length} on the device</span>
                <span class="secend">
                    {#if library}
                        <button
                            type="button"
                            class="btn primary"
                            disabled={pullJob.running || pullable.length === 0}
                            onclick={() => void pullAll()}
                        >
                            ⤓&nbsp; Pull all to library
                        </button>
                    {/if}
                </span>
            </div>

            {#if dashboard.rides.length === 0}
                <p class="small muted">No rides on the device.</p>
            {:else}
                <RideTiles
                    rides={dashboard.rides}
                    {heldHere}
                    trackFor={(id) => deviceThumbs.get("ride", Number(id))}
                    busy={previewing !== null}
                    pulling={pullJob.running}
                    onopen={(ride) => void previewRide(ride)}
                    onpull={library ? (ride) => void pullOne(ride) : null}
                />
            {/if}

            <p class="small faint disclosure">
                Rides are renamed and deleted on the device itself — this page only reads them, and copying one
                to the library does not tell the device anything.
            </p>

            {#if pullJob.running || pullJob.result || pullJob.error}
                <TransferBar job={pullJob} />
            {/if}
        </section>

        <section class="card">
            <FirmwareCard {client} info={session.info} />
        </section>

        <!-- The identity line, demoted to a footer: reference data for a bug report, not a
             headline. Selectable on purpose. -->
        <p class="identity small faint mono">
            {#if session.info}
                {session.info.hardwareRevision} · fw {session.info.firmwareRevision} · serial {session.info.serialNumber}
            {/if}
            {#if session.store}
                · card {session.store.storeId}
            {/if}
        </p>

        {#if tripDrop}
            <TripDropDialog
                files={tripDrop}
                onadd={(routes, name) => void addRoutes(routes, name)}
                oncancel={() => (tripDrop = null)}
            />
        {/if}

        {#if preview}
            {@const open = preview}
            <PreviewModal
                title={open.title}
                points={open.points}
                segments={open.segments}
                stats={open.stats}
                waypoints={open.waypoints}
                onclose={() => (preview = null)}
            >
                {#snippet actions()}
                    {#if open.route}
                        {@const route = open.route}
                        <button
                            type="button"
                            class="btn ghost"
                            onclick={() => {
                                preview = null;
                                void deleteRoute(route);
                            }}
                        >
                            Delete route…
                        </button>
                    {/if}
                {/snippet}
            </PreviewModal>
        {/if}
    {:else}
        <section class="card empty">
            <svg viewBox="0 0 24 24" width="34" height="34" aria-hidden="true">
                <circle cx="7" cy="16" r="4.4" fill="none" stroke="var(--ink-faint)" stroke-width="1.6" />
                <circle cx="17" cy="16" r="4.4" fill="none" stroke="var(--ink-faint)" stroke-width="1.6" />
                <path
                    d="M7 16 L10.2 8.5 H15 M15 8.5 L17 16 M7 16 L12.4 16 L10.2 8.5"
                    fill="none"
                    stroke="var(--ink-faint)"
                    stroke-width="1.6"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                />
            </svg>
            <!-- Driven by the session states that actually exist: idle = nothing attached,
                 connecting = the watcher saw it and is opening the link, error = it saw it and
                 could not. -->
            {#if session?.status === "connecting"}
                <p class="big">Device detected — connecting…</p>
                <p class="small muted">The link is being opened; this takes a moment.</p>
            {:else if session?.status === "error"}
                <p class="big">The device could not be opened</p>
                {#if session.error}
                    <p class="small muted">{session.error}</p>
                {/if}
                <p class="small faint">
                    Unplug it and plug it back in, or use Connect in the header to try again.
                </p>
            {:else}
                <p class="big">No device detected</p>
                <p class="small muted">
                    Plug the OpenBikeComputer in over USB — it will appear here by itself.
                </p>
                {#if platform.usbViaWebUsb}
                    <!-- WebUSB only: a device this browser has never been granted is invisible
                         until the chooser runs, and the chooser needs a click. -->
                    <p class="small faint">
                        First time in this browser? Click Connect in the header to grant access.
                    </p>
                {/if}
            {/if}
        </section>
    {/if}

    {#if platform.name === "desktop"}
        <p class="small faint disclosure cache-control">
            Route and ride previews are kept locally to avoid downloading them again after every restart.
            <button type="button" class="btn ghost" onclick={clearSavedPreviews}>Delete saved previews</button>
            {#if thumbnailNotice}<span role="status">{thumbnailNotice}</span>{/if}
        </p>
    {/if}
</article>

<style>
    article {
        width: min(920px, 100%);
        margin: 0 auto;
        display: flex;
        flex-direction: column;
        gap: 18px;
        padding-bottom: 8px;
    }

    .idrow {
        display: flex;
        align-items: baseline;
        gap: 12px;
        flex-wrap: wrap;
    }

    h1 {
        font-family: var(--serif);
        font-size: 22px;
        margin: 0;
    }

    .mono {
        font-family: var(--mono);
    }

    section {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    /* The routes/rides seam: extra air plus a quiet hairline, no chrome. */
    section.rides {
        margin-top: 14px;
        padding-top: 20px;
        border-top: 1px solid var(--line-strong);
    }

    .secline {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin: 0;
    }

    .secline h3 {
        margin: 0;
        font-size: 17px;
    }

    .secend {
        margin-left: auto;
        display: flex;
        gap: 8px;
        align-items: center;
    }

    .disclosure {
        margin: 0;
    }

    .cache-control {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 8px;
    }

    .identity {
        margin: 0;
        font-size: 11px;
        text-align: center;
        user-select: text;
    }

    .empty {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 8px;
        padding: 48px 24px;
        text-align: center;
    }

    .big {
        font-family: var(--serif);
        font-size: 18px;
    }

    .note {
        margin: 0;
        padding: 8px 12px;
        border-radius: 11px;
        background: rgba(227, 173, 51, 0.18);
        border: 1px solid var(--amber);
        line-height: 1.4;
    }

    .note.error {
        background: transparent;
        border-color: var(--coral);
        color: var(--coral);
    }

    .empty p,
    article p {
        margin: 0;
    }
</style>
