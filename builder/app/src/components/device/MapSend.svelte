<!--
  Direct set transfer plus standalone .obcm files obtained elsewhere.

  The card meter and the write progress are ONE bar (2026-08-09 P1 cleanup):
  amber is the share of the connected card this map needs, green is how much of
  the job is done, filling the amber span. Two stacked bars told the same story
  twice and made the step read as clutter; one bar is the whole picture — how
  big, how far, how much room.
-->
<script lang="ts">
    import { onMount } from "svelte";
    import { formatBytes, formatDuration, formatRate } from "../../lib/format";
    import { DeviceJob } from "../../lib/device/job.svelte";
    import { sendMapFile, type SendAssembledMap } from "../../lib/device/write";
    import { fitsOnCard, OVERHEAD_BUDGET, type Ledger } from "../../lib/catalog/ledger";
    import type { ProtocolClient } from "../../lib/usb/client";
    import TransferBar from "./TransferBar.svelte";

    let {
        client,
        ledger = null,
        sendAssembled = null,
    }: {
        client: ProtocolClient;
        /** The current cell selection, for the connected-card fit decision. */
        ledger?: Ledger | null;
        /** Step 3's backpressured assembler sink. Present only in the cell builder. */
        sendAssembled?: SendAssembledMap | null;
    } = $props();

    const job = new DeviceJob("map");
    let picker = $state<HTMLInputElement>();
    let cardFree = $state<number | null | undefined>(undefined);
    let cardError = $state<string | null>(null);
    let cardPending = $state(false);
    /** What the running job carries: the priced selection (meter applies) or a
     *  picked file whose size the ledger knows nothing about (plain bar). */
    let sending = $state<"selection" | "file" | null>(null);

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
    /** The green fill: job progress, drawn inside the amber span so one bar
     *  says "this far through this much of the card". */
    const donePct = $derived(
        job.running && job.total > 0 ? Math.min(needPct, (job.done / job.total) * needPct) : 0,
    );

    const PHASES: Record<string, string> = {
        reading: "Reading",
        downloading: "Downloading cells",
        assembling: "Assembling the map",
        sending: "Writing to the device",
        committing: "Finishing on the device",
    };

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

    async function sendSelection(send: SendAssembledMap) {
        sending = "selection";
        await job.run(
            (ctx) => send(client, ctx),
            (result) => `Map set ${result.objectId} is on the device. Restart it to switch to the new map.`,
        );
        sending = null;
        void refreshCardSpace();
    }
    async function sendFile(file: File) {
        sending = "file";
        await job.run(
            (ctx) => sendMapFile(client, file, ctx),
            (result) => `${file.name} is on the device (map ${result.objectId}). Restart it to switch to the new map.`,
        );
        sending = null;
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

    {#if ledger && sendAssembled}
        <div class="card-space small">
            <div class="space-line">
                {#if job.running && sending === "selection"}
                    <span class="muted">
                        {PHASES[job.phase] ?? "Working"}{job.partTotal
                            ? ` · ${job.partLabel ?? `shard ${job.partCurrent} of ${job.partTotal}`}`
                            : ""}
                    </span>
                    <span class="faint nums">
                        {formatBytes(job.done)}{job.total ? ` of ${formatBytes(job.total)}` : ""}{job.rate
                            ? ` · ${formatRate(job.rate)}`
                            : ""}{job.etaSeconds ? ` · about ${formatDuration(job.etaSeconds)} left` : ""}
                    </span>
                {:else}
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
                {/if}
            </div>
            {#if typeof cardFree === "number" && requiredBytes !== null}
                <div
                    class="space-bar"
                    aria-hidden="true"
                    title={`this map's share of the card${job.running && sending === "selection" ? " — green is written" : ""}`}
                >
                    <span class="need" class:short={!cardFit?.fits} style:width={`${needPct}%`}></span>
                    {#if job.running && sending === "selection"}
                        <span class="written" style:width={`${donePct}%`}></span>
                    {/if}
                </div>
            {/if}
            {#if job.running && sending === "selection"}
                <div class="cancel-line">
                    <button type="button" class="btn ghost" onclick={() => job.cancel()}>Cancel</button>
                </div>
            {:else if cardFit && !cardFit.fits}
                <p class="warn">Free {formatBytes(cardFit.shortfallBytes)} more before sending this selection.</p>
            {:else if cardError}
                <p class="warn">Card space could not be read: {cardError}</p>
            {/if}
        </div>
    {/if}

    <div class="actions">
        {#if ledger && sendAssembled}
            {@const send = sendAssembled}
            <button
                type="button"
                class="btn primary"
                disabled={job.running || !ledger.isFinal || !cardFit?.fits}
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
            A regional map is hundreds of megabytes — expect minutes, and keep the cable in.
        </p>
    {/if}

    <!-- While a selection send runs, the unified meter above carries its
         progress and the bar component would draw the same story a second
         time. A file send (whose size the ledger does not price) keeps the
         plain bar — and once any job settles, `sending` is null again, so the
         bar component is what renders the error / done notes. -->
    {#if sending !== "selection"}
        <TransferBar {job} />
    {/if}
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

    .space-line .nums {
        font-variant-numeric: tabular-nums;
        text-align: right;
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

    .space-bar .written {
        background: var(--forest);
        transition: width 0.2s linear;
    }

    @media (prefers-reduced-motion: reduce) {
        .space-bar .written {
            transition: none;
        }
    }

    .cancel-line {
        display: flex;
        justify-content: flex-end;
        margin-top: 6px;
    }

    .cancel-line .btn {
        padding: 2px 8px;
        font-size: 12px;
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
