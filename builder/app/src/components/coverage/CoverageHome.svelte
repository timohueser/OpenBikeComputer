<script lang="ts">
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
    import type { SendAssembledMap } from "../../lib/device/write";

    let {
        client,
        rootBody,
        active = true,
    }: { client: CatalogClient; rootBody: string; active?: boolean } = $props();

    // Constructed once for the component's lifetime, from props that never
    // change after mount (the home remounts this component per catalog).
    // svelte-ignore state_referenced_locally
    const store = new CoverageStore(client, rootBody);

    const partCount = $derived(store.selection.parts.length);
    let downloadStep = $state<{ sendToDevice: SendAssembledMap }>();
    let sendReady = $state(false);
    const sendAssembled: SendAssembledMap = (device, ctx) => {
        if (!downloadStep) throw new Error("The map assembler is not ready yet.");
        return downloadStep.sendToDevice(device, ctx);
    };
</script>

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
                    <span class="small muted">{store.lightSkin.name} / {store.darkSkin.name}</span>
                </summary>
                <div class="style-picker"><SkinStep {store} /></div>
            </details>
            <DownloadStep bind:this={downloadStep} {store} onSendReadyChange={(ready) => (sendReady = ready)} />
        </section>

        <section class="card">
            <div class="step-head">
                <h3>Or send directly to device</h3>
            </div>
            {#if available("deviceDashboard")}
                <MapSendStep ledger={store.ledger} {sendAssembled} {sendReady} />
            {:else}
                <DeviceStep ledger={store.ledger} {sendAssembled} {sendReady} />
            {/if}
        </section>
    </div>
</div>

<style>
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
