<!--
  Drop a GPX on the page, look at what it is, then send it.

  The conversion runs first and the *route* is what gets described — distance, ascent and point
  count read back out of the OBCR header the converter just wrote, not guessed from the GPX. So
  what the panel shows is what the device will show, and a file that converts to something
  unexpected is caught before it is on the card rather than on a hill.
-->
<script lang="ts">
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { prepareRoute, type PreparedRoute } from "../../lib/device/route";
    import { sendRoute } from "../../lib/device/write";
    import { initConvert } from "../../lib/convert/bridge";
    import type { ProtocolClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let { client }: { client: ProtocolClient } = $props();

    const job = new DeviceJob("route");
    let route = $state<PreparedRoute | null>(null);
    let readError = $state<string | null>(null);
    let dragging = $state(false);
    let picker = $state<HTMLInputElement>();

    async function accept(file: File) {
        route = null;
        readError = null;
        job.reset();
        try {
            route = await prepareRoute(file);
        } catch (cause) {
            readError = cause instanceof Error ? cause.message : String(cause);
        }
    }

    function onDrop(event: DragEvent) {
        event.preventDefault();
        dragging = false;
        const file = event.dataTransfer?.files?.[0];
        if (file) void accept(file);
    }

    function onPick(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        input.value = "";
        if (file) void accept(file);
    }

    async function send(prepared: PreparedRoute) {
        await job.run(
            (ctx) => sendRoute(client, prepared, ctx),
            (result) => `“${prepared.header.name}” is on the device (route ${result.objectId}).`,
        );
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

<section class="block">
    <h4>Route</h4>

    <!-- Dropping is a pointer-only affordance, so the group is *labelled* rather than made
         focusable: a tab stop that only accepts a drag would be a keyboard trap with nothing
         behind it. The button inside is the keyboard and screen-reader path, and it does the
         same thing. -->
    <div
        class="drop"
        role="group"
        aria-label="Route file"
        class:over={dragging}
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
        <p class="small muted">Drop a GPX file here</p>
        <button type="button" class="btn" onclick={() => picker?.click()}>Choose a file…</button>
        <input
            bind:this={picker}
            type="file"
            accept=".gpx,application/gpx+xml"
            hidden
            aria-hidden="true"
            tabindex="-1"
            onchange={onPick}
        />
    </div>

    {#if readError}
        <p class="note error small" role="alert">{readError}</p>
    {/if}

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
                <button type="button" class="btn ghost" disabled={job.running} onclick={() => (route = null)}>
                    Discard
                </button>
            </div>
        </div>
    {/if}

    <TransferBar {job} />
</section>

<style>
    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .drop {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-wrap: wrap;
        padding: 12px;
        border: 1.5px dashed var(--line-strong);
        border-radius: 10px;
        background: color-mix(in srgb, var(--parchment) 55%, transparent);
    }

    .drop.over {
        border-color: var(--forest);
        background: color-mix(in srgb, var(--forest) 8%, var(--parchment));
    }

    .drop p {
        margin: 0;
        margin-right: auto;
    }

    .picked {
        margin-top: 10px;
    }

    .picked p {
        margin: 0;
    }

    .name {
        font-family: var(--serif);
        font-size: 16px;
    }

    .actions {
        display: flex;
        gap: 8px;
        margin-top: 8px;
    }

    .note {
        margin: 8px 0 0;
    }

    .error {
        color: var(--coral);
    }
</style>
