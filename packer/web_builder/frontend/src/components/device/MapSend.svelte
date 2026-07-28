<!--
  Put a map on the device.

  Three sources, one path. A catalog artifact is fetched from the CDN and staged before a byte
  reaches the device (`OBCC_Spec.md` §7: verify the size and the SHA-256 first); a `.obcm` the
  rider already has is read straight off disk; and — on a tier that builds maps and drives USB
  itself — the map this app just built goes disk → endpoint inside Rust without entering the
  webview at all (E3 #913). None is ever held in memory — see `lib/device/staging.ts` for why that
  takes a scratch file and not a Blob.

  The built map is listed first when it exists, because it is the one the rider came here for: the
  flow the desktop tier exists to make one click is *build a map, plug in, send*. It is offered
  only when `localFileSource` is present, which is a property of the transport rather than a host
  name — a browser has no paths, so on the hosted tier this row simply does not exist.

  The line about minutes is not an apology, it is the specification: throughput is bounded by the
  SD card, so a country map takes a while and saying so up front is the difference between a slow
  transfer and a broken-looking one.

  Every success line ends with "restart it" (#927), and that is not padding either. A committed map
  is recorded as the device's selected one, but the device parses its map tables once at boot and
  streams from that map for the whole session — so until it restarts it is still showing the old
  one. Saying only "is on the device" would be true and would still read as a bug.
-->
<script lang="ts">
    import { formatBytes } from "../../lib/format";
    import { builtMap, type BuiltMap } from "../../lib/device/built.svelte";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { opfsStaging } from "../../lib/device/staging";
    import { sendCatalogMap, sendLocalMap, sendMapFile, type MapArtifact } from "../../lib/device/write";
    import type { ProtocolClient } from "../../lib/usb/client";
    import type { LocalFileSource } from "../../lib/usb/session";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        artifact = null,
        localFileSource = null,
    }: {
        client: ProtocolClient;
        /** The selected region's baked map, when one is selected. C1 (#900) owns the catalog and
         *  the picker; this component only needs the four facts an upload turns on. */
        artifact?: MapArtifact | null;
        /** The session's disk-to-endpoint path, where the transport has one. Null is not a
         *  failure: it is what "this tier is a browser tab" looks like from here. */
        localFileSource?: LocalFileSource | null;
    } = $props();

    const job = new DeviceJob("map");
    let picker = $state<HTMLInputElement>();

    // Both halves have to be true: something was built, and this transport can read it. Either
    // alone offers a button that cannot work.
    const built = $derived(localFileSource ? builtMap.current : null);

    async function sendBuilt(map: BuiltMap, open: LocalFileSource) {
        await job.run(
            (ctx) => sendLocalMap(client, map, open, ctx),
            (result) => `${map.filename} is on the device (map ${result.objectId}). Restart it to switch to the new map.`,
        );
    }

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
            (result) => `${entry.filename} is on the device (map ${result.objectId}). Restart it to switch to the new map.`,
        );
    }

    async function sendFile(file: File) {
        await job.run(
            (ctx) => sendMapFile(client, file, ctx),
            (result) => `${file.name} is on the device (map ${result.objectId}). Restart it to switch to the new map.`,
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

    {#if built}
        <p class="what small">
            <span class="name">{built.filename}</span>
            <span class="faint">{formatBytes(built.bytes)}</span>
            <span class="faint">· built here</span>
        </p>
    {:else if artifact}
        <p class="what small">
            <span class="name">{artifact.filename}</span>
            <span class="faint">{formatBytes(artifact.bytes)}</span>
        </p>
    {/if}

    <div class="actions">
        {#if built && localFileSource}
            {@const open = localFileSource}
            {@const map = built}
            <button
                type="button"
                class="btn primary"
                disabled={job.running}
                onclick={() => void sendBuilt(map, open)}
            >
                Send to device
            </button>
        {/if}
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
