<!--
  Sending a map over USB: a map is ONE `.obcm` object. The builder can stream
  its verified OPFS-backed result straight into PUT; the picker remains for a
  map the rider already has. A card's active lowest-id map is replaced with
  LIST's current revision; only a card with no map creates a new object.

  There is no free-space meter, and that is a decision rather than an omission.
  §5.2.2 retires the card-space query: nothing asks in advance, because §3.6
  answers at the point it matters — a `PUT` that does not fit is refused with
  `noSpace`, whose context is the bytes required, and the transfer bar below
  shows that sentence. A meter would be a second answer to the same question,
  and a stale one by the time the last byte arrives.
-->
<script lang="ts">
    import { onDestroy } from "svelte";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { sendMapFile, type SendAssembledMap } from "../../lib/device/write";
    import type { Ledger } from "../../lib/catalog/ledger";
    import type { FlatStoreClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        ledger = null,
        sendAssembled = null,
        sendReady = false,
    }: {
        client: FlatStoreClient;
        ledger?: Ledger | null;
        sendAssembled?: SendAssembledMap | null;
        sendReady?: boolean;
    } = $props();

    const job = new DeviceJob("map");
    onDestroy(() => job.cancel());
    let picker = $state<HTMLInputElement>();

    async function sendFile(file: File) {
        await job.run(
            (ctx) => sendMapFile(client, file, ctx),
            (result) => `${file.name} is on the device (map ${result.objectId}). Restart it to load this map.`,
        );
    }

    async function sendSelection(send: SendAssembledMap) {
        await job.run(
            (ctx) => send(client, ctx),
            (result) => `The assembled map is on the device (map ${result.objectId}). Restart it to load this map.`,
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

    <div class="actions">
        {#if ledger && sendAssembled}
            {@const send = sendAssembled}
            <button
                type="button"
                class="btn primary"
                disabled={job.running || !sendReady}
                onclick={() => void sendSelection(send)}
            >
                Assemble &amp; send map
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

    {#if !job.running}
        <p class="small faint hint">
            This replaces the active map. A regional map is hundreds of megabytes — expect minutes, and keep the cable in.
        </p>
    {/if}

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

    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }

    .hint {
        margin: 8px 0 0;
    }
</style>
