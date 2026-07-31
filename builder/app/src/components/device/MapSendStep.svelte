<!--
  Step 4 on a tier with a device page: manual single-file compatibility until #1044.

  The connection state lives in the header chip and routes/rides/firmware live on the Device tab,
  so this card is what remains of `DeviceStep` once everything with a better home has moved there —
  the send-the-map-you-just-built moment, kept beside the build button on purpose (the flow the
  desktop tier will replace with direct assembled-set transfer).

  Same chunk discipline as `DeviceStep`: this component is in the entry graph, `MapSend` drags the
  protocol client, so `MapSend` arrives through a memoized dynamic import once a device is ready.
-->
<script lang="ts">
    import { deviceHolder } from "../../lib/device/session.svelte";
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
        <MapSend client={session.client} />
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
