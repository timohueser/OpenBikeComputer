<!--
  The ride library, as a logbook (E2 #912; #894 redesign, the "Logbook" option): the list on the
  left, a sticky all-rides map on the right, and the pull that fills both.

  The desktop tier's answer to a real gap — an Android or no-phone rider has no way to get a ride
  off the device today. Not a second sync product: there is no editing here, no analysis, and no
  upload to Strava or Komoot (#781, which the iOS companion owns). It copies rides into a folder
  of GPX files, shows what is in it, and keeps those GPX files existing.

  ## The folder is GPX-only, and the GPX repairs itself

  The visible folder holds one `.gpx` per ride and nothing else; the device's own ride objects and
  the index live in app data (`apps/obc-desktop/src/rides.rs` owns that split). So there is no
  export button anywhere: the GPX exists because the ride does, and a missing one (deleted,
  renamed, moved away) is quietly re-written from the archived object on the next open or pull.
  What remains per ride is: click → preview, and "Show in folder". A ride whose *archive* is
  missing is the one thing a re-export cannot fix — that needs the device, and its row says so.

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
    import { onMount, untrack } from "svelte";
    import RideMap from "./RideMap.svelte";
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
    import { initConvert } from "../../lib/convert/bridge";

    let {
        rides = null,
        library,
        scope = null,
        onpreview = null,
    }: {
        /** The device's ride reads + ack. Null while no device is connected — the library still
         *  renders and repairs; only the pull needs a cable. */
        rides?: RideSyncSource | null;
        library: RideLibrary;
        scope?: RideScope | null;
        /** Open a preview of an archived ride (from disk, no cable). Null disables row clicks. */
        onpreview?: ((ride: LibraryRide) => void) | null;
    } = $props();

    const job = new DeviceJob("rides");

    let view = $state<LibraryView | null>(null);
    let error = $state<string | null>(null);
    let report = $state<PullReport | null>(null);
    let revealing = $state(false);
    /** The highlighted ride and which side started it — a map hover scrolls the list, a list
     *  hover must not scroll the list out from under the pointer. */
    let hovered = $state<{ key: string; from: "list" | "map" } | null>(null);
    /** `$state` so `bind:this` targets a reactive property (Svelte warns otherwise); the scroll
     *  effect reads it under `untrack`, so row churn never re-runs it. */
    const rowEls = $state<Record<string, HTMLLIElement | undefined>>({});

    /** The row thumbnail, in the SVG's own units. Small on purpose: it is a glance, not a map. */
    const PREVIEW = { width: 96, height: 54 };

    onMount(() => {
        // The wasm GPX exporter is ~95 KB and both the pull and the auto-repair end in it;
        // starting it beside the first index read turns the conversion into a plain call later.
        void initConvert();
        void refresh().then(() => repairGpx());
    });

    async function refresh() {
        try {
            view = await library.view();
            error = null;
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        }
    }

    /**
     * Quietly re-write every missing GPX from its archived object — sequential, non-blocking,
     * run on open and after every pull. Nothing crosses the cable: the archive each GPX is
     * derived from is already on this disk, which is also why a pull does not re-download a ride
     * whose GPX someone deleted.
     */
    let repairing = false;
    async function repairGpx() {
        if (repairing) return;
        const wanting = (view?.rides ?? []).filter((ride) => ride.present && !ride.gpxPresent);
        if (wanting.length === 0) return;
        repairing = true;
        const failures: string[] = [];
        try {
            for (const ride of wanting) {
                try {
                    await reexportGpx(library, ride);
                } catch (cause) {
                    failures.push(
                        `${ride.name || `Ride ${ride.objectId}`} — ${cause instanceof Error ? cause.message : String(cause)}`,
                    );
                }
            }
        } finally {
            repairing = false;
        }
        if (failures.length > 0) {
            error = `Some GPX files could not be re-written: ${failures.join("; ")}`;
        }
        await refresh();
    }

    async function pull() {
        const source = rides;
        const from = scope;
        if (!source || !from) return;
        report = null;
        const result = await job.run(
            (ctx) => pullRides(source, library, from, ctx),
            (value) => describe(value),
        );
        await refresh();
        if (result) report = result;
        void repairGpx();
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

    /** Reveal the ride's GPX in the file manager, re-exporting it first if it went missing. */
    async function showInFolder(ride: LibraryRide) {
        revealing = true;
        error = null;
        try {
            const path = ride.gpxPresent ? ride.gpxPath : await reexportGpx(library, ride);
            if (!ride.gpxPresent) await refresh();
            await library.reveal(path);
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
        } finally {
            revealing = false;
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
    /** Rides whose *archive* is gone: the one loss a local re-export cannot repair. */
    const missingArchive = $derived(listed.filter((ride) => !ride.present).length);
    const totalKm = $derived(Math.round(listed.reduce((sum, r) => sum + r.distanceM, 0) / 1000));

    /** What the map draws: every ride with an archive and a drawable track. */
    const mapRides = $derived(
        listed
            .filter((ride) => ride.present && ride.track.length > 1)
            .map((ride) => ({ key: ride.key, name: ride.name || `Ride ${ride.objectId}`, track: ride.track })),
    );

    const byKey = $derived(new Map(listed.map((ride) => [ride.key, ride])));

    function openByKey(key: string) {
        const ride = byKey.get(key);
        if (ride && ride.present) onpreview?.(ride);
    }

    // A hover that started on the map brings its row into view; one that started on the list
    // must leave the list alone.
    $effect(() => {
        const hot = hovered;
        untrack(() => {
            if (hot?.from === "map") rowEls[hot.key]?.scrollIntoView({ block: "nearest" });
        });
    });

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
            when(ride.startTime),
            rideDistance(ride.distanceM),
            rideDuration(ride.movingTimeS),
            `${ride.climbM.toLocaleString()} m up`,
        ].join(" · ");
    }
</script>

<section class="block">
    <div class="head">
        <h4>Ride library</h4>
        <span class="count small faint">
            {listed.length} ride{listed.length === 1 ? "" : "s"}{#if totalKm > 0}&nbsp;· {totalKm.toLocaleString()} km
                total{/if}
        </span>
        <span class="headend">
            <button type="button" class="link small faint" onclick={() => view && void library.reveal(view.folder)}>
                {view?.folder ?? "…"}
            </button>
            <button type="button" class="btn ghost small-btn" onclick={() => void relocate()}>Change…</button>
            {#if rides && scope}
                <button type="button" class="btn primary" disabled={job.running} onclick={() => void pull()}>
                    ⤓&nbsp; Pull rides from device
                </button>
            {:else}
                <span class="small faint">Plug the device in to pull new rides.</span>
            {/if}
        </span>
    </div>

    <div class="logbook">
        <div class="listcol">
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
                    No rides here yet. Plug the device in and pull — each ride lands as a GPX file in the
                    folder above, where you can back it up or drag it into anything that reads GPX.
                </p>
            {:else}
                <ul class="rides">
                    {#each listed as ride (ride.key)}
                        {@const path = trackPath(ride.track, PREVIEW.width, PREVIEW.height)}
                        <li
                            class:missing={!ride.present}
                            class:hot={hovered?.key === ride.key}
                            bind:this={rowEls[ride.key]}
                            onmouseenter={() => (hovered = { key: ride.key, from: "list" })}
                            onmouseleave={() => (hovered = null)}
                        >
                            <button
                                type="button"
                                class="open"
                                disabled={!ride.present || !onpreview}
                                onclick={() => onpreview?.(ride)}
                            >
                                <span class="preview" aria-hidden="true">
                                    {#if path}
                                        <svg viewBox="0 0 {PREVIEW.width} {PREVIEW.height}" role="presentation">
                                            <path d={path} />
                                        </svg>
                                    {/if}
                                </span>
                                <span class="what">
                                    <span class="name">{ride.name || `Ride ${ride.objectId}`}</span>
                                    <span class="small faint">{facts(ride)}</span>
                                    {#if !ride.present}
                                        <span class="small warn">
                                            not on this computer any more — pull from the device to restore it
                                        </span>
                                    {/if}
                                </span>
                            </button>
                            <button
                                type="button"
                                class="iconbtn"
                                title="Show in folder"
                                aria-label="Show in folder"
                                disabled={revealing || (!ride.present && !ride.gpxPresent)}
                                onclick={() => void showInFolder(ride)}
                            >
                                📁
                            </button>
                            <span class="chev" aria-hidden="true">›</span>
                        </li>
                    {/each}
                </ul>

                {#if missingArchive > 0}
                    <p class="small faint">
                        {missingArchive} ride{missingArchive === 1 ? "" : "s"} listed here
                        {missingArchive === 1 ? "is" : "are"} no longer on this computer — pull from the
                        device to restore {missingArchive === 1 ? "it" : "them"}.
                    </p>
                {/if}
            {/if}

            <!--
              The disclosure. Deliberately below the list and always present: a rider who learns this
              after the device has deleted a ride has learned it too late.
            -->
            <p class="small muted disclosure">
                Copying a ride here tells the device a durable copy exists off it. That is what lets you
                delete the ride on the device without a warning — and it is also what starts the device's
                auto-delete countdown for that ride. Check <strong>Settings → Rides</strong> on the device
                for how long it keeps a synced ride; the app cannot read that setting over the cable. The
                copies in this folder are yours and are never deleted by this app.
            </p>

            <TransferBar {job} />
        </div>

        <div class="mapcol">
            <RideMap
                rides={mapRides}
                hovered={hovered?.key ?? null}
                onhover={(key) => (hovered = key ? { key, from: "map" } : null)}
                onopen={openByKey}
            />
        </div>
    </div>
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
        margin-bottom: 10px;
        flex-wrap: wrap;
    }

    .count {
        white-space: nowrap;
    }

    .headend {
        margin-left: auto;
        display: flex;
        align-items: center;
        gap: 8px;
        min-width: 0;
    }

    .link {
        border: 0;
        background: none;
        padding: 0;
        font: inherit;
        color: inherit;
        text-align: right;
        text-decoration: underline;
        text-decoration-style: dotted;
        cursor: pointer;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        max-width: 320px;
    }

    /* --- the two columns: ledger left, sticky map right --- */

    .logbook {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(320px, 42%);
        gap: 14px;
        align-items: start;
    }

    .mapcol {
        position: sticky;
        top: 0;
        height: calc(100dvh - var(--head-h) - 120px);
        min-height: 280px;
    }

    @media (max-width: 940px) {
        .logbook {
            grid-template-columns: minmax(0, 1fr);
        }

        /* Stacked: the map first, at a fixed height, and no stickiness — the page scrolls. */
        .mapcol {
            order: -1;
            position: static;
            height: 300px;
        }
    }

    /* --- the ledger --- */

    .rides {
        list-style: none;
        margin: 0 0 10px;
        padding: 0;
    }

    .rides li {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 4px 8px 4px 0;
        border-radius: 10px;
        transition: background 0.12s;
    }

    .rides li + li {
        border-top: 1px solid var(--line);
        border-radius: 0 0 10px 10px;
    }

    .rides li.hot {
        background: color-mix(in srgb, var(--coral) 8%, transparent);
    }

    .rides li.missing .preview {
        opacity: 0.35;
    }

    .open {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 6px 0;
        background: none;
        border: 0;
        text-align: left;
        color: inherit;
        font: inherit;
    }

    .open:disabled {
        cursor: default;
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
        min-width: 0;
        display: flex;
        flex-direction: column;
    }

    .name {
        font-family: var(--serif);
        font-size: 16px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .warn {
        color: var(--coral);
    }

    .chev {
        flex: none;
        color: var(--ink-faint);
        font-size: 18px;
        line-height: 1;
        padding-right: 2px;
    }

    .disclosure {
        margin-top: 10px;
    }

    .note {
        margin: 8px 0;
    }

    .error {
        color: var(--coral);
    }
</style>
