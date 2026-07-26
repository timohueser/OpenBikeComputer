<!--
  Put a map on the device.

  Two sources, one path. A catalog artifact is fetched from the CDN and staged before a byte
  reaches the device (`OBCC_Spec.md` §7: verify the size and the SHA-256 first); a `.obcm` the
  rider already has is read straight off disk. Neither is ever held in memory — see
  `lib/device/staging.ts` for why that takes a scratch file and not a Blob.

  The line about minutes is not an apology, it is the specification: throughput is bounded by the
  SD card, so a country map takes a while and saying so up front is the difference between a slow
  transfer and a broken-looking one.
-->
<script lang="ts">
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { opfsStaging } from "../../lib/device/staging";
    import { sendCatalogMap, sendMapFile, type MapArtifact } from "../../lib/device/write";
    import type { ProtocolClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        artifact = null,
    }: {
        client: ProtocolClient;
        /** The selected region's baked map, when one is selected. C1 (#900) owns the catalog and
         *  the picker; this component only needs the four facts an upload turns on. */
        artifact?: MapArtifact | null;
    } = $props();

    const job = new DeviceJob();
    let picker = $state<HTMLInputElement>();

    async function sendArtifact(entry: MapArtifact) {
        const area = opfsStaging();
        if (!area) {
            job.phase = "error";
            job.error =
                "This browser will not give the page a scratch file to hold the download. " +
                "Download the map and send it as a file instead.";
            return;
        }
        await job.run(
            (ctx) => sendCatalogMap(client, entry, area, ctx),
            (result) => `${entry.filename} is on the device (map ${result.objectId}).`,
        );
    }

    async function sendFile(file: File) {
        await job.run(
            (ctx) => sendMapFile(client, file, ctx),
            (result) => `${file.name} is on the device (map ${result.objectId}).`,
        );
    }

    function onPick(event: Event) {
        const input = event.currentTarget as HTMLInputElement;
        const file = input.files?.[0];
        // Clear it so picking the same file twice still fires a change event.
        input.value = "";
        if (file) void sendFile(file);
    }
</script>

<section class="block">
    <h4>Map</h4>

    {#if artifact}
        <p class="what small">
            <span class="name">{artifact.filename}</span>
            <span class="faint">{formatBytes(artifact.bytes)}</span>
        </p>
    {/if}

    <div class="actions">
        {#if artifact}
            <button
                type="button"
                class="btn primary"
                disabled={job.running}
                onclick={() => artifact && void sendArtifact(artifact)}
            >
                Send map to device
            </button>
        {/if}
        <button
            type="button"
            class="btn"
            disabled={job.running}
            onclick={() => picker?.click()}
        >
            Send a .obcm file…
        </button>
        <input
            bind:this={picker}
            type="file"
            accept=".obcm"
            hidden
            aria-hidden="true"
            tabindex="-1"
            onchange={onPick}
        />
    </div>

    <p class="small faint hint">
        Maps write at the card's speed, not the cable's — a regional map takes several minutes.
    </p>

    <TransferBar {job} />
</section>

<style>
    .block + :global(.block) {
        margin-top: 16px;
    }

    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    .what {
        margin: 0 0 8px;
        display: flex;
        gap: 8px;
        align-items: baseline;
    }

    .name {
        font-family: var(--mono);
    }

    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }

    .hint {
        margin: 8px 0 0;
    }
</style>
