<!--
  The device page: what is on the card, listed and touchable — the thumbdrive view (#894 epic,
  restructure of 2026-07-28).

  This route is loaded through a dynamic import (`App.svelte`), which is what lets it reach the
  protocol client and codecs directly: nothing here may leak into the entry chunk, and nothing
  here needs to — the session already exists in `deviceHolder`, opened by the header chip.

  Division of labour with the cards: the cards render lists and take snippets; every operation
  that touches the cable lives here, funneled through `dashboard.enqueue` so the page cannot trip
  the client's one-transfer rule over itself.
-->
<script lang="ts">
    import FirmwareCard from "../components/device/FirmwareCard.svelte";
    import PreviewModal from "../components/device/PreviewModal.svelte";
    import RidesCard from "../components/device/RidesCard.svelte";
    import RouteDrop from "../components/device/RouteDrop.svelte";
    import RoutesCard from "../components/device/RoutesCard.svelte";
    import TransferBar from "../components/device/TransferBar.svelte";
    import TripDropDialog from "../components/device/TripDropDialog.svelte";
    import { routeTrack } from "../lib/convert/bridge";
    import { dashboard, type TripView } from "../lib/device/dashboard.svelte";
    import type { ProfilePoint } from "../lib/device/elevation";
    import { pullRide, pullRides, rideSyncAccess, type LibraryView, type RideLibrary } from "../lib/device/library";
    import { addStage, createTrip, moveStage, removeStage, renameRoute, updateTrip } from "../lib/device/manage";
    import { rideDistance, rideDuration, rideScope } from "../lib/device/rides";
    import type { PreparedRoute } from "../lib/device/route";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { DeviceJob, jobRegistry } from "../lib/device/job.svelte";
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

    /** One mutation, queued, with the refresh that makes the card the authority again. */
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

    async function addRoutes(routes: PreparedRoute[], tripName: string | null) {
        const c = client;
        tripDrop = null;
        if (!c) return;
        await dropJob.run(
            async (ctx) => {
                // Sequential — the wire takes one transfer at a time. The device dedupes a
                // re-dropped file by CRC and answers with the existing id, so the collected
                // ids are correct even when half of these were already on the card.
                const ids: number[] = [];
                for (const route of routes) {
                    const { objectId } = await dashboard.enqueue(() => sendRoute(c, route, ctx));
                    ids.push(objectId);
                }
                if (tripName !== null) {
                    await dashboard.enqueue(() => createTrip(c, tripName, ids, ctx.signal));
                }
                return { count: ids.length, tripName };
            },
            (r) =>
                r.tripName !== null
                    ? `“${r.tripName}” is on the device, ${r.count} stages.`
                    : `${r.count} routes are on the device.`,
        );
        await dashboard.refresh(c);
    }

    // --- the library's view of the card's rides ------------------------------
    //
    // Only for the badges and the pulls: `platform.rides` exists exactly on the tier that also
    // has the Ride-library page. Loaded once, refreshed after every pull.

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

    // --- previews: download the object, decode it, show it. Never acks. ----

    let preview = $state<{
        title: string;
        points: ProfilePoint[];
        stats: Array<{ label: string; value: string }>;
        /** Set for routes: the modal's footer offers Delete. */
        route: RouteListEntry | null;
    } | null>(null);
    /** The object a preview download is running for, to mark the busy row. */
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

        <RoutesCard
            onpreview={(route) => void previewRoute(route)}
            onrename={doRenameRoute}
            ondelete={(route) => void deleteRoute(route)}
            onaddtotrip={doAddToTrip}
            onrenametrip={doRenameTrip}
            ondeletetrip={(trip) => void deleteTrip(trip)}
            onremovestage={(trip, index) => void doRemoveStage(trip, index)}
            onmovestage={doMoveStage}
        />
        <section class="card">
            <RouteDrop
                {client}
                {serialize}
                onmultiple={(files) => (tripDrop = files)}
                onsent={() => client && void dashboard.refresh(client)}
            />
            <TransferBar job={dropJob} />
        </section>
        <RidesCard {heldHere}>
            {#snippet actions()}
                {#if library}
                    <button
                        type="button"
                        class="btn primary"
                        disabled={pullJob.running || dashboard.rides.length === 0}
                        onclick={() => void pullAll()}
                    >
                        Pull all to library
                    </button>
                {/if}
            {/snippet}
            {#snippet row(ride)}
                <button
                    type="button"
                    class="btn"
                    disabled={previewing !== null}
                    onclick={() => void previewRide(ride)}
                >
                    Preview
                </button>
                {#if library}
                    <button
                        type="button"
                        class="btn"
                        disabled={pullJob.running || (heldHere?.has(ride.objectId) ?? false)}
                        onclick={() => void pullOne(ride)}
                    >
                        Pull
                    </button>
                {/if}
            {/snippet}
        </RidesCard>
        {#if pullJob.running || pullJob.result || pullJob.error}
            <TransferBar job={pullJob} />
        {/if}
        <section class="card">
            <FirmwareCard {client} info={session.info} />
        </section>

        {#if tripDrop}
            <TripDropDialog
                files={tripDrop}
                onadd={(routes, tripName) => void addRoutes(routes, tripName)}
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
        gap: 14px;
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
