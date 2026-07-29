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
    import { routeTrack } from "../lib/convert/bridge";
    import { dashboard, type TripView } from "../lib/device/dashboard.svelte";
    import type { ProfilePoint } from "../lib/device/elevation";
    import {
        previewTrack,
        pullRide,
        pullRides,
        rideSyncAccess,
        type LibraryView,
        type RideLibrary,
    } from "../lib/device/library";
    import { addStage, createTrip, moveStage, removeStage, renameRoute, updateTrip } from "../lib/device/manage";
    import { rideDistance, rideDuration, rideScope } from "../lib/device/rides";
    import type { PreparedRoute } from "../lib/device/route";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { DeviceJob, jobRegistry } from "../lib/device/job.svelte";
    import {
        deviceThumbs,
        rideFingerprint,
        routeFingerprint,
        type Thumb,
        type ThumbRequest,
    } from "../lib/device/thumbs.svelte";
    import { formatBytes } from "../lib/format";
    import { platform } from "../lib/platform";
    import { confirmAction } from "../lib/ui/confirm.svelte";
    import { ObjectType } from "../lib/usb/protocol";
    import type { ProtocolClient } from "../lib/usb/client";
    import { decodeRideObject, type RideListEntry, type RouteListEntry } from "../lib/usb/objects";
    import { sendRoute } from "../lib/device/write";

    const session = $derived(deviceHolder.session);
    const client = $derived(session?.status === "ready" ? session.client : null);
    const scope = $derived(rideScope(session?.info ?? null, session?.identity ?? null));

    // Load once per (serial, epoch); the store survives tab switches, so coming
    // back renders instantly and a card swap reloads.
    $effect(() => {
        if (client) void dashboard.ensureLoaded(client, scope);
    });

    /** One mutation, queued, with the refresh that makes the page the authority again. */
    async function mutate(op: (client: ProtocolClient) => Promise<unknown>): Promise<void> {
        const c = client;
        if (!c) return;
        try {
            await dashboard.enqueue(() => op(c));
            await dashboard.refresh(c);
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        }
    }

    const doRenameRoute = (route: RouteListEntry, name: string) =>
        void mutate((c) => renameRoute(c, route.objectId, name));

    const doRenameTrip = (trip: TripView, name: string) =>
        void mutate((c) => updateTrip(c, trip.objectId, (t) => ({ ...t, name })));

    const doAddToTrip = (route: RouteListEntry, tripId: number | null) =>
        void mutate((c) =>
            tripId === null
                ? createTrip(c, route.name || `Route ${route.objectId}`, [route.objectId])
                : updateTrip(c, tripId, (t) => addStage(t, route.objectId)),
        );

    /** The ⋯ menu's "Keep on device" pick — §4.4 cmd 6, then the refresh shows the new tag. */
    const doSetRetention = (route: RouteListEntry, level: number) =>
        void mutate((c) => c.setRouteRetention(route.objectId, level));

    const doMoveStage = (trip: TripView, index: number, delta: number) =>
        void mutate((c) => updateTrip(c, trip.objectId, (t) => moveStage(t, index, delta)));

    async function doRemoveStage(trip: TripView, index: number) {
        // Removing the last stage would leave an empty grouping — offer to take
        // the trip with it instead of leaving a husk on the card.
        if ((trip.detail?.stages.length ?? 0) <= 1) {
            const ok = await confirmAction({
                title: `Remove the last route from “${trip.name}”?`,
                body: "An empty trip is nothing, so the trip is deleted with it. The route stays on the device.",
                confirmLabel: "Remove and delete trip",
                destructive: true,
            });
            if (!ok) return;
            await mutate((c) => c.deleteObject(ObjectType.Trip, trip.objectId));
            return;
        }
        await mutate((c) => updateTrip(c, trip.objectId, (t) => removeStage(t, index)));
    }

    async function deleteRoute(route: RouteListEntry) {
        if (!client) return;
        const ok = await confirmAction({
            title: `Delete “${route.name || `Route ${route.objectId}`}” from the device?`,
            body: "The route is removed from the card. A copy on this computer, if you have one, is not touched.",
            confirmLabel: "Delete route",
            destructive: true,
        });
        if (!ok) return;
        await mutate((c) => c.deleteObject(ObjectType.Route, route.objectId));
    }

    async function deleteTrip(trip: TripView) {
        if (!client) return;
        const ok = await confirmAction({
            title: `Delete the trip “${trip.name || `Trip ${trip.objectId}`}”?`,
            body: "Only the grouping is removed — its routes stay on the device as ordinary routes.",
            confirmLabel: "Delete trip",
            destructive: true,
        });
        if (!ok) return;
        await mutate((c) => c.deleteObject(ObjectType.Trip, trip.objectId));
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

    async function addRoutes(routes: PreparedRoute[], tripName: string | null, retention: number) {
        const c = client;
        tripDrop = null;
        if (!c) return;
        await dropJob.run(
            async (ctx) => {
                // Sequential — the wire takes one transfer at a time. The device dedupes a
                // re-dropped file by CRC and answers with the existing id, so the collected
                // ids are correct even when half of these were already on the card — and the
                // chosen retention lands on those too, which is the spec's case (b), an edit.
                //
                // The retention stamp is best-effort per stage: the uploads and the trip are
                // the substance, and a failed annotation on stage k must not strand stages
                // k+1…n off the card. Failures are counted and said once, in the result line.
                const ids: number[] = [];
                let retentionFailures = 0;
                for (const route of routes) {
                    const { objectId } = await dashboard.enqueue(() => sendRoute(c, route, ctx));
                    ids.push(objectId);
                    if (retention !== 0) {
                        try {
                            await dashboard.enqueue(() => c.setRouteRetention(objectId, retention, ctx.signal));
                        } catch (cause) {
                            // A cancel is the rider's, not a stamp failure — stop the whole job.
                            if (ctx.signal.aborted) throw cause;
                            retentionFailures += 1;
                        }
                    }
                }
                if (tripName !== null) {
                    await dashboard.enqueue(() => createTrip(c, tripName, ids, ctx.signal));
                }
                return { count: ids.length, tripName, retentionFailures };
            },
            (r) => {
                const landed =
                    r.tripName !== null
                        ? `“${r.tripName}” is on the device, ${r.count} stages.`
                        : `${r.count} routes are on the device.`;
                if (r.retentionFailures === 0) return landed;
                const which = r.retentionFailures === 1 ? "one stage" : `${r.retentionFailures} stages`;
                return `${landed} The keep-on-device setting could not be applied to ${which} — set it from the route's ⋯ menu.`;
            },
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

    /** Ride ids a durable copy of which is in the folder — inverse of the library's own list. */
    const heldHere = $derived.by(() => {
        if (!libraryView || scope.epoch === null) return null;
        return new Set(
            libraryView.rides
                .filter((r) => r.present && r.serial === scope.serial && r.epoch === scope.epoch)
                .map((r) => r.objectId),
        );
    });

    async function refreshLibrary() {
        if (library) libraryView = await library.view();
    }

    async function pullOne(entry: RideListEntry) {
        const c = client;
        const lib = library;
        if (!c || !lib) return;
        await pullJob.run(
            (ctx) => dashboard.enqueue(() => pullRide(rideSyncAccess(c), lib, scope, entry, ctx)),
            ({ ride }) => `“${ride.name}” is in the library.`,
        );
        await refreshLibrary();
    }

    async function pullAll() {
        const c = client;
        const lib = library;
        if (!c || !lib) return;
        await pullJob.run(
            (ctx) => dashboard.enqueue(() => pullRides(rideSyncAccess(c), lib, scope, ctx)),
            (report) =>
                report.imported.length === 0 && report.repaired.length === 0
                    ? `Nothing new — all ${report.listed} rides on the device are already in the library.`
                    : `Copied ${report.imported.length + report.repaired.length} to the library.`,
        );
        await refreshLibrary();
    }

    // --- thumbnails: one small download per object, ever, behind everything else ----

    function routeThumbRequest(c: ProtocolClient, route: RouteListEntry): ThumbRequest {
        return {
            kind: "route",
            id: route.objectId,
            fingerprint: routeFingerprint(route),
            load: async (signal) => {
                const points = await routeTrack(await c.download(ObjectType.Route, route.objectId, { signal }));
                return points.map((p) => [p.lat, p.lon] as [number, number]);
            },
        };
    }

    function rideThumbRequest(c: ProtocolClient, ride: RideListEntry, held: Thumb | undefined): ThumbRequest {
        return {
            kind: "ride",
            id: ride.objectId,
            fingerprint: rideFingerprint(ride),
            // A ride already pulled has its preview track in the library index — the free win:
            // no download, and the tile shows exactly what the Ride-library page shows.
            load: held
                ? async () => held
                : async (signal) =>
                      previewTrack(
                          decodeRideObject(await c.download(ObjectType.Ride, ride.objectId, { signal })),
                      ),
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
            ...dashboard.routes.map((route) => routeThumbRequest(c, route)),
            ...dashboard.rides.map((ride) => rideThumbRequest(c, ride, heldTracks.get(ride.objectId))),
        ];
        const aborter = new AbortController();
        // `untrack`: the fill reads (and writes) the thumb store's reactive map, and this effect
        // must re-run on list changes, not on every thumbnail that lands.
        void untrack(() => deviceThumbs.fill(scope, requests, serialize, aborter.signal));
        // Unmount or disconnect: stop walking, and abort the download in flight.
        return () => aborter.abort();
    });

    // --- previews: download the object, decode it, show it. Never acks. ----

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
        stats: Array<{ label: string; value: string }>;
        /** Set for routes: the modal's footer offers Delete. */
        route: RouteListEntry | null;
    } | null>(null);
    /** The object a preview download is running for, to mark the page busy. */
    let previewing = $state<string | null>(null);

    async function previewRoute(route: RouteListEntry) {
        const c = client;
        if (!c || previewing) return;
        previewing = `route-${route.objectId}`;
        try {
            const obcr = await dashboard.enqueue(() => c.download(ObjectType.Route, route.objectId));
            preview = {
                title: route.name || `Route ${route.objectId}`,
                points: await routeTrack(obcr),
                stats: [
                    { label: "Distance", value: `${(route.distanceM / 1000).toFixed(1)} km` },
                    { label: "Ascent", value: `${route.ascentM.toLocaleString()} m` },
                    {
                        label: "Points · waypoints",
                        value: `${route.pointCount.toLocaleString()} · ${route.waypointCount}`,
                    },
                    { label: "Size on card", value: formatBytes(route.byteLen) },
                ],
                route,
            };
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            previewing = null;
        }
    }

    async function previewRide(ride: RideListEntry) {
        const c = client;
        if (!c || previewing) return;
        previewing = `ride-${ride.objectId}`;
        try {
            const object = decodeRideObject(
                await dashboard.enqueue(() => c.download(ObjectType.Ride, ride.objectId)),
            );
            preview = {
                title: ride.name || `Ride ${ride.objectId}`,
                points: object.points.map((p) => ({ lat: p.lat1e7 / 1e7, lon: p.lon1e7 / 1e7, ele: p.eleM })),
                stats: [
                    { label: "Distance", value: rideDistance(ride.distanceM) },
                    { label: "Moving time", value: rideDuration(ride.movingTimeS) },
                    { label: "Avg speed", value: `${((ride.avgSpeedCms / 100) * 3.6).toFixed(1)} km/h` },
                    { label: "Climb", value: `${ride.climbM.toLocaleString()} m` },
                ],
                route: null,
            };
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            previewing = null;
        }
    }

    /**
     * Preview a whole trip: its stages' thumbnail tracks, concatenated, with the trip's totals as
     * the stats. The tracks are the same downsampled ones the band draws — mostly already in the
     * store or the cache, fetched through the queue where not — so this opens without downloading
     * anything the tiles did not already need. No elevation: the profile simply stays away.
     */
    async function previewTrip(trip: TripView) {
        const c = client;
        if (!c || previewing) return;
        const stages = dashboard.stagesOf(trip).filter((s): s is { id: number; route: RouteListEntry } => s.route !== null);
        if (stages.length === 0) return;
        previewing = `trip-${trip.objectId}`;
        try {
            const points: ProfilePoint[] = [];
            for (const stage of stages) {
                const track = await deviceThumbs.ensure(
                    scope,
                    routeThumbRequest(c, stage.route),
                    serialize,
                    pageLifetime.signal,
                );
                for (const [lat, lon] of track) points.push({ lat, lon, ele: null });
            }
            preview = {
                title: trip.name || `Trip ${trip.objectId}`,
                points,
                stats: [
                    { label: "Stages", value: `${stages.length}` },
                    { label: "Distance", value: `${(trip.totalDistanceM / 1000).toFixed(1)} km` },
                    { label: "Ascent", value: `${trip.totalAscentM.toLocaleString()} m` },
                ],
                route: null,
            };
        } catch (cause) {
            dashboard.error = cause instanceof Error ? cause.message : String(cause);
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
            <span class="small faint mono">
                {#if session.info}
                    {session.info.hardwareRevision} · fw {session.info.firmwareRevision} · serial {session.info.serialNumber}
                {/if}
                {#if session.identity?.obcmVersion != null}
                    · maps v{session.identity.obcmVersion}
                {/if}
            </span>
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
                    trackFor={(id) => deviceThumbs.get("route", id)}
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
                trackFor={(id) => deviceThumbs.get("route", id)}
                busy={previewing !== null}
                onopen={(route) => void previewRoute(route)}
                onrename={doRenameRoute}
                ondelete={(route) => void deleteRoute(route)}
                onaddtotrip={doAddToTrip}
                onsetretention={doSetRetention}
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

        <section>
            <div class="secline">
                <h3>Rides</h3>
                <span class="small faint">{dashboard.rides.length} on the device</span>
                <span class="secend">
                    {#if library}
                        <button
                            type="button"
                            class="btn primary"
                            disabled={pullJob.running || dashboard.rides.length === 0}
                            onclick={() => void pullAll()}
                        >
                            ⤓&nbsp; Pull all to library
                        </button>
                    {/if}
                </span>
            </div>

            {#if dashboard.ridesTruncated}
                <p class="small faint">
                    The device listed its newest rides only; older ones are still on the card.
                </p>
            {/if}

            {#if dashboard.rides.length === 0}
                <p class="small muted">No rides on the device.</p>
            {:else}
                <RideTiles
                    rides={dashboard.rides}
                    {heldHere}
                    trackFor={(id) => deviceThumbs.get("ride", id)}
                    busy={previewing !== null}
                    pulling={pullJob.running}
                    onopen={(ride) => void previewRide(ride)}
                    onpull={library ? (ride) => void pullOne(ride) : null}
                />
            {/if}

            <p class="small faint disclosure">
                Rides are renamed and deleted on the device itself — over the cable they are read-only, so a
                ride can never be lost.
            </p>

            {#if pullJob.running || pullJob.result || pullJob.error}
                <TransferBar job={pullJob} />
            {/if}
        </section>

        <section class="card">
            <FirmwareCard {client} info={session.info} />
        </section>

        {#if tripDrop}
            <TripDropDialog
                files={tripDrop}
                onadd={(routes, tripName, retention) => void addRoutes(routes, tripName, retention)}
                oncancel={() => (tripDrop = null)}
            />
        {/if}

        {#if preview}
            {@const open = preview}
            <PreviewModal
                title={open.title}
                points={open.points}
                stats={open.stats}
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
            <p class="big">No device connected</p>
            <p class="small muted">
                Plug the OpenBikeComputer in over USB — it will appear here by itself.
            </p>
            {#if session?.status === "connecting"}
                <p class="small faint">Connecting…</p>
            {/if}
        </section>
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
