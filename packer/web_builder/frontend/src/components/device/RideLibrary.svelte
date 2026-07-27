<!--
  The ride library: the folder, the list, and the pull that fills it (E2, #912).

  The desktop tier's answer to a real gap — an Android or no-phone rider has no way to get a ride
  off the device today. Not a second sync product: there is no editing here, no analysis, and no
  upload to Strava or Komoot (#781, which the iOS companion owns). It copies rides into a folder,
  shows what is in it, and writes GPX.

  ## The one sentence that has to be on screen

  Pulling a ride tells the device a durable copy exists here — that is what `synced` means (spec
  §4.4), and it is what starts the device's auto-delete countdown for that ride (#638). A rider who
  does not know that can be surprised by a ride disappearing from the device a week later. So the
  disclosure is a permanent line under the list, not a tooltip, and it says both halves: what the
  ack buys (the device stops warning about deleting the ride) and what it costs (the countdown
  starts).

  It cannot report the *device's* retention setting, and says so rather than guessing: the Config
  object on the wire (§7.3) carries a name and a units flag and nothing else, so ride retention is
  readable only on the device itself. Inventing "1 week" here because that is the firmware default
  would be a number the app cannot stand behind.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import TransferBar from "./TransferBar.svelte";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import {
        pullRides,
        reexportGpx,
        trackPath,
        type LibraryRide,
        type LibraryView,
        type PullReport,
        type RideLibrary,
        type RideSyncSource,
    } from "../../lib/device/library";
    import { rideDistance, rideDuration, type RideScope } from "../../lib/device/rides";
    import { formatBytes } from "../../lib/format";
    import { initConvert } from "../../lib/convert/bridge";

    let {
        rides,
        library,
        scope,
    }: { rides: RideSyncSource; library: RideLibrary; scope: RideScope } = $props();

    const job = new DeviceJob();

    let view = $state<LibraryView | null>(null);
    let error = $state<string | null>(null);
    let report = $state<PullReport | null>(null);
    let exporting = $state(false);

    /** The preview box, in the SVG's own units. Small on purpose: it is a glance, not a map. */
    const PREVIEW = { width: 96, height: 54 };

    onMount(() => {
        // The wasm GPX exporter is ~95 KB and every pull ends in it; starting it beside the first
        // index read turns the conversion into a plain function call later.
        void initConvert();
        void refresh();
    });

    async function refresh() {
        try {
            view = await library.view();
            error = null;
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        }
    }

    async function pull() {
        report = null;
        const result = await job.run(
            (ctx) => pullRides(rides, library, scope, ctx),
            (value) => describe(value),
        );
        await refresh();
        if (result) report = result;
    }

    function describe(value: PullReport): string {
        if (value.imported.length === 0 && value.repaired.length === 0) {
            return `Nothing new — all ${value.listed} ride${value.listed === 1 ? "" : "s"} on the device are already here.`;
        }
        const parts: string[] = [];
        if (value.imported.length) parts.push(`${value.imported.length} new`);
        if (value.repaired.length) parts.push(`${value.repaired.length} restored`);
        return `Copied ${parts.join(" and ")} to ${view?.folder ?? "the library"}.`;
    }

    /**
     * Re-write every ride's GPX from its stored object — the bulk export, and the repair.
     *
     * Nothing crosses the cable: the archive each GPX is derived from is already in the folder,
     * which is why a pull does not re-download a ride whose GPX someone deleted.
     */
    async function exportAll() {
        if (!view) return;
        exporting = true;
        error = null;
        try {
            for (const ride of view.rides) {
                if (ride.present) await reexportGpx(library, ride);
            }
            await refresh();
            await library.reveal(view.folder);
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            exporting = false;
        }
    }

    async function exportOne(ride: LibraryRide) {
        exporting = true;
        error = null;
        try {
            const path = ride.gpxPresent ? ride.gpxPath : await reexportGpx(library, ride);
            await refresh();
            await library.reveal(path);
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            exporting = false;
        }
    }

    async function relocate() {
        try {
            const picked = await library.chooseFolder();
            if (picked) await refresh();
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        }
    }

    /** Newest first — the order rides are read in. */
    const listed = $derived(
        [...(view?.rides ?? [])].sort((a, b) => b.startTime - a.startTime || b.importedAt - a.importedAt),
    );
    const missing = $derived(listed.filter((ride) => !ride.present).length);

    /** The device's ride ids this library already holds — what the Pull button can promise. */
    const heldHere = $derived(
        new Set(
            listed
                .filter((ride) => ride.present && ride.serial === scope.serial && ride.epoch === scope.epoch)
                .map((ride) => ride.objectId),
        ),
    );

    function when(startTime: number): string {
        if (!startTime) return "date not recorded";
        // UTC, like the filename: the ride object's `start_time` is UTC seconds, and a late-evening
        // ride would otherwise be filed on the wrong day west of Greenwich.
        return new Date(startTime * 1000).toLocaleDateString(undefined, {
            year: "numeric",
            month: "short",
            day: "numeric",
            timeZone: "UTC",
        });
    }

    function facts(ride: LibraryRide): string {
        return [
            rideDistance(ride.distanceM),
            rideDuration(ride.movingTimeS),
            `${ride.climbM.toLocaleString()} m up`,
            `${ride.points.toLocaleString()} points`,
        ].join(" · ");
    }
</script>

<section class="block">
    <div class="head">
        <h4>Ride library</h4>
        <button type="button" class="btn primary" disabled={job.running} onclick={() => void pull()}>
            Pull rides from device
        </button>
    </div>

    <p class="small faint folder">
        <button type="button" class="link" onclick={() => view && void library.reveal(view.folder)}>
            {view?.folder ?? "…"}
        </button>
        <button type="button" class="btn ghost small-btn" onclick={() => void relocate()}>Change…</button>
    </p>

    {#if error}
        <p class="note error small" role="alert">{error}</p>
    {/if}

    {#if report && report.failed.length > 0}
        <p class="note error small" role="alert">
            {report.failed.length} ride{report.failed.length === 1 ? "" : "s"} could not be saved and
            {report.failed.length === 1 ? "was" : "were"} left on the device:
            {report.failed.map((f) => `${f.name} — ${f.message}`).join("; ")}
        </p>
    {/if}
    {#if report?.truncated}
        <p class="small faint">
            The device listed its newest rides only; older ones are still on the card and were not
            pulled.
        </p>
    {/if}

    {#if listed.length === 0}
        <p class="small muted">
            No rides here yet. Plug the device in and pull — the files land in the folder above,
            where you can back them up or drag them into anything that reads GPX.
        </p>
    {:else}
        <ul class="rides">
            {#each listed as ride (ride.key)}
                {@const path = trackPath(ride.track, PREVIEW.width, PREVIEW.height)}
                <li class:missing={!ride.present}>
                    <div class="preview" aria-hidden="true">
                        {#if path}
                            <svg viewBox="0 0 {PREVIEW.width} {PREVIEW.height}" role="presentation">
                                <path d={path} />
                            </svg>
                        {/if}
                    </div>
                    <div class="what">
                        <p class="name">
                            {ride.name || `Ride ${ride.objectId}`}
                            {#if heldHere.has(ride.objectId) && ride.epoch === scope.epoch}
                                <span class="tag">on device</span>
                            {/if}
                        </p>
                        <p class="small faint">{when(ride.startTime)} · {facts(ride)}</p>
                        <p class="small faint">
                            {#if ride.present}
                                {formatBytes(ride.bytes)} archived{ride.gpxPresent ? " · GPX" : " · no GPX yet"}
                            {:else}
                                <span class="warn">the file is no longer in this folder</span>
                            {/if}
                        </p>
                    </div>
                    <button
                        type="button"
                        class="btn"
                        disabled={exporting || !ride.present}
                        onclick={() => void exportOne(ride)}
                    >
                        {ride.gpxPresent ? "Show GPX" : "Export GPX"}
                    </button>
                </li>
            {/each}
        </ul>

        <div class="bulk">
            <button type="button" class="btn ghost" disabled={exporting} onclick={() => void exportAll()}>
                Export all as GPX
            </button>
            {#if missing > 0}
                <span class="small faint">
                    {missing} ride{missing === 1 ? "" : "s"} listed here {missing === 1 ? "has" : "have"} no
                    file left in the folder — pull again to fetch {missing === 1 ? "it" : "them"} back.
                </span>
            {/if}
        </div>
    {/if}

    <!--
      The disclosure. Deliberately below the list and always present: a rider who learns this after
      the device has deleted a ride has learned it too late.
    -->
    <p class="small muted disclosure">
        Copying a ride here tells the device a durable copy exists off it. That is what lets you
        delete the ride on the device without a warning — and it is also what starts the device's
        auto-delete countdown for that ride. Check <strong>Settings → Rides</strong> on the device
        for how long it keeps a synced ride; the app cannot read that setting over the cable. The
        copies in this folder are yours and are never deleted by this app.
    </p>

    <TransferBar {job} />
</section>

<style>
    h4 {
        margin: 0;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .head {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 6px;
    }

    .head :global(.btn) {
        margin-left: auto;
    }

    .folder {
        display: flex;
        align-items: center;
        gap: 8px;
        margin: 0 0 10px;
        min-width: 0;
    }

    .link {
        border: 0;
        background: none;
        padding: 0;
        font: inherit;
        color: inherit;
        text-align: left;
        text-decoration: underline;
        text-decoration-style: dotted;
        cursor: pointer;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .rides {
        list-style: none;
        margin: 0 0 10px;
        padding: 0;
    }

    .rides li {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 10px 0;
    }

    .rides li + li {
        border-top: 1px solid var(--line);
    }

    .rides li.missing .preview {
        opacity: 0.35;
    }

    .preview {
        flex: 0 0 auto;
        width: 96px;
        height: 54px;
        border: 1px solid var(--line);
        background: var(--paper-2, transparent);
    }

    .preview svg {
        display: block;
        width: 100%;
        height: 100%;
    }

    .preview path {
        fill: none;
        stroke: var(--ink);
        stroke-width: 1.4;
        stroke-linejoin: round;
        stroke-linecap: round;
    }

    .what {
        margin-right: auto;
        min-width: 0;
    }

    .what p {
        margin: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 16px;
    }

    .tag {
        margin-left: 6px;
        font-family: var(--sans);
        font-size: 11px;
        letter-spacing: 0.04em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .warn {
        color: var(--coral);
    }

    .bulk {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
    }

    .disclosure {
        margin-top: 10px;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }
</style>
