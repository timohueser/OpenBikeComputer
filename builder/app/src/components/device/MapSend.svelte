<!--
  Sending a map over USB: a map is ONE `.obcm` file, so this is one picker and
  one transfer.

  The card meter above it prices the *selection* the builder is working on
  against the connected card — how big, and how much room is left — so a rider
  finds out a map will not fit before they spend minutes on the cable rather
  than after.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { formatBytes } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { sendMapFile } from "../../lib/device/write";
    import { fitsOnCard, OVERHEAD_BUDGET, type Ledger } from "../../lib/catalog/ledger";
    import type { ProtocolClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        ledger = null,
    }: {
        client: ProtocolClient;
        /** The current cell selection, for the connected-card fit decision. */
        ledger?: Ledger | null;
    } = $props();

    const job = new DeviceJob("map");
    let picker = $state<HTMLInputElement>();
    let cardFree = $state<number | null | undefined>(undefined);
    let cardError = $state<string | null>(null);
    let cardPending = $state(false);

    const requiredBytes = $derived(ledger ? Math.ceil(ledger.totalBytes * (1 + OVERHEAD_BUDGET)) : null);
    const cardFit = $derived(
        ledger && typeof cardFree === "number" ? fitsOnCard(ledger, cardFree) : null,
    );

    /** The amber span: this map's share of the connected card. */
    const needPct = $derived(
        requiredBytes !== null && typeof cardFree === "number" && cardFree > 0
            ? Math.min(100, (requiredBytes / cardFree) * 100)
            : 0,
    );

    async function refreshCardSpace() {
        cardPending = true;
        cardError = null;
        try {
            cardFree = await client.cardFreeBytes();
        } catch (cause) {
            cardFree = undefined;
            cardError = cause instanceof Error ? cause.message : String(cause);
        } finally {
            cardPending = false;
        }
    }

    onMount(() => void refreshCardSpace());

    async function sendFile(file: File) {
        await job.run(
            (ctx) => sendMapFile(client, file, ctx),
            (result) => `${file.name} is on the device (map ${result.objectId}). Restart it to switch to the new map.`,
        );
        void refreshCardSpace();
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

    {#if ledger}
        <div class="card-space small">
            <div class="space-line">
                <span>Connected card</span>
                {#if cardPending}
                    <span class="faint">checking space…</span>
                {:else if typeof cardFree === "number" && requiredBytes !== null}
                    <span class:warn={!cardFit?.fits}>
                        {formatBytes(requiredBytes)} needed · {formatBytes(cardFree)} free
                    </span>
                {:else if cardFree === null}
                    <span class="warn">no readable card</span>
                {:else}
                    <button type="button" class="retry" onclick={() => void refreshCardSpace()}>check again</button>
                {/if}
            </div>
            {#if typeof cardFree === "number" && requiredBytes !== null}
                <div class="space-bar" aria-hidden="true" title="this map's share of the card">
                    <span class="need" class:short={!cardFit?.fits} style:width={`${needPct}%`}></span>
                </div>
            {/if}
            {#if cardFit && !cardFit.fits}
                <p class="warn">Free {formatBytes(cardFit.shortfallBytes)} more before sending this selection.</p>
            {:else if cardError}
                <p class="warn">Card space could not be read: {cardError}</p>
            {/if}
        </div>
    {/if}

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

    .card-space {
        margin: 0 0 10px;
        padding: 8px 10px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--parchment);
    }

    .space-line {
        display: flex;
        justify-content: space-between;
        gap: 12px;
    }

    .space-bar {
        position: relative;
        height: 6px;
        margin-top: 6px;
        border-radius: 3px;
        background: var(--parchment-3);
        overflow: hidden;
    }

    .space-bar span {
        position: absolute;
        inset: 0 auto 0 0;
        display: block;
        height: 100%;
    }

    .space-bar .need {
        background: var(--amber);
    }

    .space-bar .need.short {
        background: var(--coral);
    }

    .warn {
        color: var(--coral);
    }

    .card-space p {
        margin: 6px 0 0;
    }

    .retry {
        border: 0;
        background: none;
        color: var(--forest-deep);
        text-decoration: underline;
        padding: 0;
    }
</style>
