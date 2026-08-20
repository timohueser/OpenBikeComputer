<!--
  The destructive recovery operation for a removable card, shared by the browser and desktop
  surfaces. FORMAT is a protocol-v4 operation rather than host-side filesystem work: the device
  owns the card, writes a fresh flat-store superblock pair, answers durably, then reboots.
-->
<script lang="ts">
    import { confirmAction } from "../../lib/ui/confirm.svelte";
    import type { FlatStoreClient } from "../../lib/usb/client";

    let { client, storeId }: { client: FlatStoreClient; storeId: string | null } = $props();

    let running = $state(false);
    let error = $state<string | null>(null);

    async function formatCard() {
        if (running) return;
        const ok = await confirmAction({
            title: "Format the device card?",
            body:
                "This permanently deletes everything on the card: its map, routes, trips, rides, " +
                "weather and update packages. The device will restart with an empty card; reconnect, " +
                "then send it a map.",
            confirmLabel: "Delete everything and format",
            destructive: true,
        });
        if (!ok) return;

        running = true;
        error = null;
        try {
            await client.format(storeId);
            // A successful FORMAT response is immediately followed by a device reboot. The owning
            // session observes that disconnect and reconnects; this component normally unmounts
            // before there is anything useful to render as a local success state.
        } catch (cause) {
            error = cause instanceof Error ? cause.message : String(cause);
            running = false;
        }
    }
</script>

<section class="block format" class:recovery={storeId === null}>
    <h4>Card</h4>
    {#if storeId === null}
        <p class="small">This card is not initialized as an OpenBikeComputer flat store.</p>
        <p class="small muted">Format it here, reconnect after the restart, then send a map.</p>
    {:else}
        <p class="small muted">Erase and initialize the card again if you want a completely clean device.</p>
    {/if}

    <button type="button" class="btn danger" disabled={running} onclick={() => void formatCard()}>
        {running ? "Formatting…" : "Format card…"}
    </button>

    {#if error}<p class="note error small" role="alert">{error}</p>{/if}
</section>

<style>
    .format.recovery {
        padding: 14px;
        border: 1px solid var(--coral);
        border-radius: 8px;
    }

    h4 {
        margin: 0 0 6px;
        font-size: 14px;
        font-family: var(--sans);
        letter-spacing: 0.02em;
        text-transform: uppercase;
        color: var(--ink-faint);
    }

    p {
        margin: 0 0 8px;
    }

    .danger {
        color: var(--coral);
        border-color: color-mix(in srgb, var(--coral) 55%, var(--line));
    }

    .error {
        color: var(--coral);
    }
</style>
