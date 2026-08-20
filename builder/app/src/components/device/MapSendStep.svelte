<!--
  The connection state lives in the header chip and routes/rides/firmware live on the Device tab,
  so this card keeps the direct send action beside the build flow.

  `MapSend` brings in the protocol client and is loaded only once a device is ready.
-->
<script lang="ts">
    import { deviceHolder } from "../../lib/device/session.svelte";
    import type { Ledger } from "../../lib/catalog/ledger";
    import type { SendAssembledMap } from "../../lib/device/write";

    let {
        ledger = null,
        sendAssembled = null,
    }: {
        ledger?: Ledger | null;
        sendAssembled?: SendAssembledMap | null;
    } = $props();

    let mapSend: Promise<typeof import("./MapSend.svelte")> | undefined;
    const loadMapSend = () => (mapSend ??= import("./MapSend.svelte"));

    const session = $derived(deviceHolder.session);
</script>

{#if deviceHolder.interrupted}
    <p class="note error small" role="alert">{deviceHolder.interrupted}</p>
{/if}

{#if session?.status === "ready" && session.client}
    {#await loadMapSend()}
        <p class="small muted">Loading…</p>
    {:then { default: MapSend }}
        <MapSend client={session.client} {ledger} {sendAssembled} />
    {:catch}
        <p class="note error small" role="alert">
            The device tools could not be loaded. Check your connection and reload the page.
        </p>
    {/await}
{:else}
    <p class="small muted">Connect a device (top right) to send a map over USB.</p>
{/if}

<style>
    .note {
        margin: 0 0 8px;
    }

    .error {
        color: var(--coral);
    }

    p {
        margin: 0;
    }
</style>
