<!--
  The ride library page: the managed folder, promoted from a card in the old device column to a
  place of its own (#894 restructure). It works with no device attached — the folder and its GPX
  exports are local — and gains the pull the moment the header chip goes green.

  Loaded through a dynamic import (`App.svelte`) on the one tier with `caps.rideLibrary`, so the
  Tauri-backed library and the codecs stay out of the entry chunk.
-->
<script lang="ts">
    import PreviewModal from "../components/device/PreviewModal.svelte";
    import RideLibraryPanel from "../components/device/RideLibrary.svelte";
    import type { ProfilePoint } from "../lib/device/elevation";
    import { rideSyncAccess, type LibraryRide, type RideLibrary } from "../lib/device/library";
    import { rideDistance, rideDuration, rideScope } from "../lib/device/rides";
    import { deviceHolder } from "../lib/device/session.svelte";
    import { platform } from "../lib/platform";
    import { decodeRideObject } from "../lib/usb/objects";

    // Non-null on every tier that can render this page (caps.rideLibrary gates the
    // route); the fallback rejection keeps the await block honest if that ever drifts.
    const managed: Promise<RideLibrary> = platform.rides
        ? platform.rides()
        : Promise.reject(new Error("this tier has no ride library"));

    const session = $derived(deviceHolder.session);
    const connected = $derived(session?.status === "ready" && session.client ? session : null);

    let preview = $state<{
        title: string;
        points: ProfilePoint[];
        stats: Array<{ label: string; value: string }>;
    } | null>(null);
    let previewError = $state<string | null>(null);

    /** Preview from the archived object on disk — full resolution, no cable. */
    async function openPreview(library: RideLibrary, ride: LibraryRide) {
        previewError = null;
        try {
            const object = decodeRideObject(await library.readObject(ride.key));
            preview = {
                title: ride.name || `Ride ${ride.objectId}`,
                points: object.points.map((p) => ({
                    lat: p.lat1e7 / 1e7,
                    lon: p.lon1e7 / 1e7,
                    ele: p.eleM,
                })),
                stats: [
                    { label: "Distance", value: rideDistance(ride.distanceM) },
                    { label: "Moving time", value: rideDuration(ride.movingTimeS) },
                    { label: "Climb", value: `${ride.climbM.toLocaleString()} m` },
                    { label: "Points", value: ride.points.toLocaleString() },
                ],
            };
        } catch (cause) {
            previewError = cause instanceof Error ? cause.message : String(cause);
        }
    }
</script>

<article>
    {#await managed}
        <section class="card"><p class="small muted">Opening the ride library…</p></section>
    {:then library}
        {#if previewError}
            <p class="note error small" role="alert">{previewError}</p>
        {/if}
        <section class="card">
            <RideLibraryPanel
                {library}
                rides={connected?.client ? rideSyncAccess(connected.client) : null}
                scope={connected ? rideScope(connected.info, connected.identity) : null}
                onpreview={(ride) => void openPreview(library, ride)}
            />
        </section>
    {:catch reason}
        <section class="card">
            <p class="note error small" role="alert">
                The ride library could not be opened ({reason instanceof Error ? reason.message : reason}).
                Rides are still on the device and nothing has been changed there.
            </p>
        </section>
    {/await}

    {#if preview}
        <PreviewModal
            title={preview.title}
            points={preview.points}
            stats={preview.stats}
            onclose={() => (preview = null)}
        />
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

    .note {
        margin: 0;
    }

    .error {
        color: var(--coral);
    }

    p {
        margin: 0;
    }
</style>
