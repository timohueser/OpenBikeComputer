<!--
  Sending a map over USB: a map is ONE `.obcm` file, so this is one picker and
  one transfer.

  There is no free-space meter, and that is a decision rather than an omission.
  §5.2.2 retires the card-space query: nothing asks in advance, because §3.6
  answers at the point it matters — a `PUT` that does not fit is refused with
  `noSpace`, whose context is the bytes required, and the transfer bar below
  shows that sentence. A meter would be a second answer to the same question,
  and a stale one by the time the last byte arrives.
-->
<script lang="ts">
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { sendMapFile } from "../../lib/device/write";
    import type { FlatStoreClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let { client }: { client: FlatStoreClient } = $props();

    const job = new DeviceJob("map");
    let picker = $state<HTMLInputElement>();

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

    <div class="actions">
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
            A regional map is hundreds of megabytes — expect minutes, and keep the cable in.
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
