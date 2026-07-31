<script lang="ts">
    // The builder's home when the catalog is a cell store (#1038): the approved
    // R2·1 frame. The steps column is the narrative spine, the map pane
    // responds, and only the column scrolls. Step 1 is a ledger of composed
    // parts and the map owns selection
    // through its tool rail.
    //
    // Step 4 uses the existing device step: sending an assembled
    // volume set over USB is the next slice (P4d), and until it lands the step
    // says what it can honestly do — manage the device, and take a map file the
    // rider already saved.

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
    const sendAssembled: SendAssembledMap = (client, ctx) => {
        if (!downloadStep) throw new Error("The map assembler is not ready yet.");
        return downloadStep.sendToDevice(client, ctx);
    };
</script>

<div class="layout">
    <CoverageMap {store} {active} />

    <div class="steps">
        <section class="card">
            <div class="step-head">
                <span class="num">1</span>
                <h3>Coverage</h3>
                {#if partCount > 0}
                    <span class="small faint">
                        {partCount}
                        {partCount === 1 ? "part" : "parts"} — add more with the map tools
                    </span>
                {/if}
            </div>
            <div class="stack">
                <PartsList {store} />
                <MapSummary {store} />
            </div>
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">2</span>
                <h3>Skin</h3>
            </div>
            <SkinStep {store} />
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">3</span>
                <h3>Download</h3>
            </div>
            <DownloadStep bind:this={downloadStep} {store} />
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">4</span>
                <h3>{available("deviceDashboard") ? "Send to device" : "Device"}</h3>
            </div>
            {#if available("deviceDashboard")}
                <MapSendStep ledger={store.ledger} {sendAssembled} />
            {:else}
                <DeviceStep ledger={store.ledger} {sendAssembled} />
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
        margin-bottom: 10px;
    }

    .step-head h3 {
        font-size: 16.5px;
    }

    .step-head .small {
        margin-left: auto;
        text-align: right;
    }

    .num {
        width: 21px;
        height: 21px;
        flex: none;
        border-radius: 50%;
        border: 1.6px solid var(--wood);
        color: var(--ink);
        font-size: 12px;
        font-weight: 600;
        display: inline-flex;
        align-items: center;
        justify-content: center;
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
