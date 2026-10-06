<script lang="ts">
    import { tick } from "svelte";
    import type { CatalogClient } from "../../lib/catalog/client";
    import { CoverageStore } from "../../lib/coverage/store.svelte";
    import { available } from "../../lib/platform/gating";
    import DeviceStep from "../device/DeviceStep.svelte";
    import MapSendStep from "../device/MapSendStep.svelte";
    import CoverageMap from "./CoverageMap.svelte";
    import DownloadStep from "./DownloadStep.svelte";
    import MapSummary from "./MapSummary.svelte";
    import PartsList from "./PartsList.svelte";
    import SkinStep from "./SkinStep.svelte";
    import { jobRegistry } from "../../lib/device/job.svelte";
    import type { SendAssembledMap } from "../../lib/device/write";

    let {
        client,
        rootBody,
        active = true,
        refreshCatalog,
    }: { client: CatalogClient; rootBody: string; active?: boolean; refreshCatalog: () => Promise<{ client: CatalogClient; body: string }> } = $props();

    // svelte-ignore state_referenced_locally
    let store = $state.raw(new CoverageStore(client, rootBody));

    const partCount = $derived(store.selection.parts.length);
    const styleNames = $derived([
        store.lightSkins.length ? store.lightSkin.name : null,
        store.darkSkins.length ? store.darkSkin.name : null,
    ].filter(Boolean).join(" / "));
    let downloadStep = $state<{ sendToDevice: SendAssembledMap; pauseForRefresh: () => Promise<() => void> }>();
    let sendReady = $state(false);
    const sendAssembled: SendAssembledMap = (device, ctx) => {
        if (!downloadStep) throw new Error("The map assembler is not ready yet.");
        return downloadStep.sendToDevice(device, ctx);
    };
    let refreshing = $state(false);
    let refreshError = $state<string | null>(null);
    let refreshNotice = $state<string | null>(null);
    let downloadFailed = $state(false);
    let runBlocked = $state(false);
    const refreshBlocked = $derived(runBlocked || jobRegistry.active !== null);
    const catalogFailed = $derived(!!store.indexError || store.regionErrors.size > 0 || !!store.resolutionError || downloadFailed);
    let recovery: HTMLDivElement | undefined = $state();

    async function refresh() {
        if (refreshing || refreshBlocked || !downloadStep) return;
        refreshing = true;
        refreshError = null;
        let resume: (() => void) | undefined;
        try {
            resume = await downloadStep.pauseForRefresh();
            const { client, body } = await refreshCatalog();
            if (jobRegistry.active) throw new Error("Wait for the device transfer to finish before refreshing.");
            store = store.refreshed(client, body);
            downloadFailed = false;
            refreshNotice = "Map catalog refreshed. Your coverage selection is kept. Check the map summary before downloading.";
        } catch (cause) {
            refreshError = cause instanceof Error ? cause.message : String(cause);
            resume?.();
        } finally {
            refreshing = false;
            await tick();
            recovery?.focus();
        }
    }
</script>

{#if catalogFailed || refreshing || refreshError || refreshNotice || store.refreshNotice}
    <div class="recovery small" bind:this={recovery} tabindex="-1">
        <p role="status">{refreshError ? `The map catalog could not be refreshed: ${refreshError}` : refreshing ? "Refreshing the map catalog…" : store.refreshNotice ?? refreshNotice ?? "Refresh the map catalog to use the current published map data. Your selection is kept."}</p>
        <button type="button" class="btn" disabled={refreshing || refreshBlocked} onclick={() => void refresh()}>Refresh map catalog</button>
        {#if refreshBlocked}<p>Wait for the device transfer or map cleanup to finish before refreshing.</p>{/if}
    </div>
{/if}

{#key store}
<div class="layout">
    <CoverageMap {store} {active} />

    <div class="steps">
        <section class="card">
            <div class="step-head">
                <h3>Choose coverage</h3>
                {#if partCount > 0}
                    <span class="small faint">
                        {partCount}
                        {partCount === 1 ? "part" : "parts"}
                    </span>
                {/if}
            </div>
            <div class="stack">
                <PartsList {store} />
                {#if partCount > 0 || store.indexError || store.resolutionError}
                    <MapSummary {store} />
                {/if}
            </div>
        </section>

        <section class="card">
            <div class="step-head">
                <h3>Download your map</h3>
            </div>
            <details class="style-options">
                <summary>
                    <span>Map style <span class="small faint">· Optional</span></span>
                    <span class="small muted">{styleNames}</span>
                </summary>
                <div class="style-picker"><SkinStep {store} /></div>
            </details>
            <DownloadStep bind:this={downloadStep} {store} onSendReadyChange={(ready) => (sendReady = ready)} onFailureChange={(failed) => (downloadFailed = failed)} onRefreshBlockedChange={(blocked) => (runBlocked = blocked)} />
        </section>

        <section class="card">
            <div class="step-head">
                <h3>Or send directly to device</h3>
            </div>
            {#if available("deviceDashboard")}
                <MapSendStep ledger={store.ledger} {sendAssembled} sendReady={sendReady && !refreshing} />
            {:else}
                <DeviceStep ledger={store.ledger} {sendAssembled} sendReady={sendReady && !refreshing} />
            {/if}
        </section>
    </div>
</div>

{/key}

<style>
    .recovery { padding: 10px 0; }
    .recovery p { margin: 0 0 8px; }

    /* The pane takes what the viewport
       gives, the steps column is the one thing that scrolls (narrow screens
       trade the lock back for page scrolling). */
    .layout {
        flex: 1;
        min-height: 0;
        display: grid;
        grid-template-columns: minmax(0, 1.5fr) minmax(330px, 1fr);
        gap: 14px;
        align-items: stretch;
    }

    .steps {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 0;
        min-height: 0;
        overflow-y: auto;
        padding-right: 4px;
    }

    .stack {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .step-head {
        display: flex;
        align-items: center;
        gap: 9px;
        margin: -16px -16px 14px;
        padding: 11px 16px;
        background: var(--parchment-2);
        border-bottom: 1px solid var(--line);
        border-radius: 9px 9px 0 0;
    }

    .step-head h3 {
        font-size: 16.5px;
    }

    .step-head .small {
        margin-left: auto;
        text-align: right;
    }

    .style-options {
        margin-bottom: 16px;
        border-bottom: 1px solid var(--line);
        padding-bottom: 14px;
    }

    .style-options summary {
        cursor: pointer;
        color: var(--ink);
        line-height: 1.6;
    }

    .style-options summary > span:last-child {
        display: block;
        margin-left: 18px;
        overflow-wrap: anywhere;
    }

    .style-picker {
        padding-top: 14px;
    }

    @media (max-width: 940px) {
        .layout {
            grid-template-columns: 1fr;
        }

        .steps {
            overflow: visible;
            padding-right: 0;
        }
    }
</style>
