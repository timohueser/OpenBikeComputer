<!--
  The GPX drop zone, as the routes grid's last, ghost tile: drop or click to choose, pick how long
  the device keeps it, send.

  The conversion runs first and the *route* is what gets described — distance, ascent and point
  count read back out of the OBCR header the converter just wrote, not guessed from the GPX. So
  what the tile shows is what the device will show, and a file that converts to something
  unexpected is caught before it is on the card rather than on a hill.

  "Keep on device" is §4.4 cmd 6: the upload itself has no retention field, so a non-forever
  choice is applied right after the commit, before the page refreshes its lists. The device
  dedupes a re-dropped file by CRC and answers with the existing id — applying the chosen
  retention to that id is the spec's case (b), an edit, which is exactly what re-dropping means.
-->
<script lang="ts">
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { prepareRoute, type PreparedRoute } from "../../lib/device/route";
    import { RETENTION_LEVELS, retentionLabel } from "../../lib/device/retention";
    import { sendRoute } from "../../lib/device/write";
    import { initConvert } from "../../lib/convert/bridge";
    import type { ProtocolClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        onmultiple = null,
        serialize = null,
        onsent = null,
        empty = false,
        heading = "Route",
    }: {
        client: ProtocolClient;
        /** Take over when several files land at once (the trip dialog). Null keeps the
         *  single-file behaviour: extra files are ignored, as they always were. */
        onmultiple?: ((files: File[]) => void) | null;
        /** Order this surface's transfers behind a page-owned queue (the device page's
         *  dashboard chain). Null sends directly, as the builder column always has. */
        serialize?: (<T>(op: () => Promise<T>) => Promise<T>) | null;
        /** A route landed — the device page refreshes its lists on this. */
        onsent?: (() => void) | null;
        /** True when the card holds no routes at all: the tile carries the empty-state line. */
        empty?: boolean;
        /** Non-null wraps the tile in the builder column's `section.block` + `h4` chrome
         *  (`DeviceSurfaces` relies on the `.block` rhythm for its separators). The device
         *  page passes null: there the grid is the chrome. */
        heading?: string | null;
    } = $props();

    const job = new DeviceJob("route");
    let route = $state<PreparedRoute | null>(null);
    let readError = $state<string | null>(null);
    let dragging = $state(false);
    /** The §4.4 cmd 6 level applied after a successful send. `0` = forever = send nothing. */
    let retention = $state(0);
    /** The upload landed but the retention stamp did not — its own sentence, never the job's. */
    let retentionNote = $state<string | null>(null);
    let picker = $state<HTMLInputElement>();

    async function accept(file: File) {
        route = null;
        readError = null;
        retentionNote = null;
        job.reset();
        try {
            route = await prepareRoute(file);
        } catch (cause) {
            readError = cause instanceof Error ? cause.message : String(cause);
        }
    }

    function take(files: File[]) {
        if (files.length === 0) return;
        if (files.length > 1 && onmultiple) {
            onmultiple(files);
            return;
        }
        void accept(files[0]);
    }

    function onDrop(event: DragEvent) {
        event.preventDefault();
        dragging = false;
        take([...(event.dataTransfer?.files ?? [])]);
    }

    function onPick(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        const files = [...(input.files ?? [])];
        input.value = "";
        take(files);
    }

    async function send(prepared: PreparedRoute) {
        const run = serialize ?? (<T,>(op: () => Promise<T>) => op());
        const keep = retention;
        retentionNote = null;
        // The job is the *upload*, alone: once the device has committed the route, the tile
        // must say so and the page must re-list, whatever happens next — a retention stamp
        // failing after the commit is an annotation problem, not a failed send.
        const result = await job.run(
            (ctx) => run(() => sendRoute(client, prepared, ctx)),
            (value) => `“${prepared.header.name}” is on the device (route ${value.objectId}).`,
        );
        if (!result) return;
        route = null;
        // Forever is the device's default for a fresh upload, so level 0 sends nothing; any
        // other choice is stamped before the refresh. The id may be a dedupe's existing route —
        // see the header.
        if (keep !== 0) {
            try {
                await run(() => client.setRouteRetention(result.objectId, keep));
            } catch {
                retentionNote =
                    "The route is on the device, but the keep-on-device setting could not be " +
                    "applied — set it from the route's ⋯ menu.";
            }
        }
        onsent?.();
    }

    /** True where the event came from an inner control the tile click must not speak over. */
    function fromInnerControl(event: Event): boolean {
        return (
            event.target instanceof Element &&
            event.target.closest("button, input, select, label") !== null
        );
    }

    function openPicker(event: Event) {
        if (fromInnerControl(event)) return;
        picker?.click();
    }

    function onKeydown(event: KeyboardEvent) {
        if (event.key !== "Enter" && event.key !== " ") return;
        if (fromInnerControl(event)) return;
        event.preventDefault();
        picker?.click();
    }

    const summary = $derived(
        route
            ? [
                  `${(route.header.distanceM / 1000).toFixed(1)} km`,
                  `${route.header.ascentM} m up`,
                  `${route.header.pointCount.toLocaleString()} points`,
                  formatBytes(route.obcr.length),
              ].join(" · ")
            : null,
    );
</script>

{#snippet tile()}
<div
    class="ghost"
    class:over={dragging}
    role="button"
    tabindex="0"
    aria-label="Add a route: drop GPX files here or press to choose"
    onclick={openPicker}
    onkeydown={onKeydown}
    ondragover={(e) => {
        e.preventDefault();
        dragging = true;
        // The wasm module is ~95 KB and loads on demand; starting it on hover turns the
        // conversion into a plain function call by the time the file lands.
        void initConvert();
    }}
    ondragleave={() => (dragging = false)}
    ondrop={onDrop}
>
    {#if route}
        <div class="picked">
            <p class="name">{route.header.name}</p>
            <p class="small faint">{summary}</p>
            <div class="actions">
                <button
                    type="button"
                    class="btn primary"
                    disabled={job.running}
                    onclick={() => route && void send(route)}
                >
                    Send route to device
                </button>
                <button type="button" class="btn ghostbtn" disabled={job.running} onclick={() => (route = null)}>
                    Discard
                </button>
            </div>
        </div>
    {:else}
        <div class="glyph" aria-hidden="true">⤓</div>
        <p class="lead">{empty ? "No routes on the device yet — drop a GPX here" : "Drop GPX here"}</p>
        <p class="small faint">
            or click to choose{#if onmultiple}&nbsp;· several files become a trip{/if}
        </p>
    {/if}

    {#if readError}
        <p class="note small" role="alert">{readError}</p>
    {/if}

    {#if retentionNote}
        <p class="note small" role="alert">{retentionNote}</p>
    {/if}

    <label class="keep small" for="routedrop-keep">
        keep on device:
        <select id="routedrop-keep" bind:value={retention} onclick={(e) => e.stopPropagation()}>
            {#each RETENTION_LEVELS as level (level)}
                <option value={level}>{retentionLabel(level)}</option>
            {/each}
        </select>
    </label>

    <input
        bind:this={picker}
        type="file"
        accept=".gpx,application/gpx+xml"
        multiple={onmultiple !== null}
        hidden
        aria-hidden="true"
        tabindex="-1"
        onchange={onPick}
    />

    <TransferBar {job} />
</div>
{/snippet}

{#if heading !== null}
    <section class="block">
        <h4>{heading}</h4>
        {@render tile()}
    </section>
{:else}
    {@render tile()}
{/if}

<style>
    /* The builder column's chrome (`DeviceSurfaces`): the `.block` rhythm and the uppercase
       heading every sibling surface carries. The device page skips both — heading = null. */
    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .ghost {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: center;
        gap: 6px;
        min-height: 176px;
        padding: 14px;
        border: 1.5px dashed var(--line-strong);
        border-radius: 14px;
        color: var(--ink-faint);
        cursor: pointer;
        text-align: center;
    }

    .ghost:hover,
    .ghost:focus-visible {
        border-color: var(--forest);
        color: var(--forest-deep);
    }

    .ghost:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }

    .ghost.over {
        border-color: var(--forest);
        background: color-mix(in srgb, var(--forest) 8%, var(--parchment));
    }

    .ghost p {
        margin: 0;
    }

    .glyph {
        font-size: 26px;
        line-height: 1;
    }

    .lead {
        font-weight: 600;
        color: var(--ink-soft);
    }

    .keep {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }

    .keep select {
        font-size: 12.5px;
        padding: 2px 6px;
        border-radius: 7px;
    }

    .picked {
        display: flex;
        flex-direction: column;
        gap: 4px;
        align-items: center;
        min-width: 0;
        max-width: 100%;
    }

    .name {
        font-family: var(--serif);
        font-size: 15.5px;
        color: var(--ink);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        max-width: 100%;
    }

    .actions {
        display: flex;
        gap: 8px;
        margin-top: 6px;
        flex-wrap: wrap;
        justify-content: center;
    }

    /* Quiet secondary — `.ghost` is taken by the tile itself. */
    .ghostbtn {
        background: transparent;
        color: var(--ink);
        border-color: var(--wood);
    }

    .note {
        color: var(--coral);
        max-width: 100%;
    }
</style>
