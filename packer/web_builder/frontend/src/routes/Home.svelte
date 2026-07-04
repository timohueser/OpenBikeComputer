<script lang="ts">
    import { onMount } from "svelte";
    import BuildCard from "../components/BuildCard.svelte";
    import MapPanel from "../components/MapPanel.svelte";
    import PresetCards from "../components/PresetCards.svelte";
    import { api } from "../lib/api/client";
    import type { Preset } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";
    import type { AreaSelection } from "../lib/map/selection";

    let { active = true }: { active?: boolean } = $props();

    let presets = $state<Preset[]>([]);
    let presetError = $state<string | null>(null);
    let selection = $state<AreaSelection | null>(null);

    onMount(async () => {
        const restored = working.restore();
        try {
            presets = await api.presets();
            // First visit: the default preset is the working config.
            if (!restored && presets.length) working.applyPreset(presets[0]);
        } catch (e) {
            presetError = e instanceof Error ? e.message : String(e);
        }
    });

    const areaSummary = $derived.by(() => {
        if (!selection) return null;
        if (selection.mode === "bbox") {
            if (!selection.bbox) return null;
            const [w, s, e, n] = selection.bbox;
            return {
                title: `Box W ${w.toFixed(3)} · S ${s.toFixed(3)} · E ${e.toFixed(3)} · N ${n.toFixed(3)}`,
                hint: selection.coveringNames.length
                    ? `≈ ${selection.areaKm2} · from ${selection.coveringNames.join(", ")}`
                    : "no downloadable region covers this area",
                warn: selection.coveringNames.length === 0,
            };
        }
        if (selection.regionIds.length === 0) return null;
        return {
            title: selection.regionNames.join(", "),
            hint: `${selection.regionIds.length} ${selection.regionIds.length === 1 ? "region" : "regions"}`,
            warn: false,
        };
    });
</script>

<div class="layout">
    <MapPanel {active} onchange={(sel) => (selection = sel)} />

    <div class="steps">
        <section class="card">
            <div class="step-head">
                <span class="num">1</span>
                <h3>Area</h3>
                {#if areaSummary}
                    <span class="small faint">{areaSummary.hint}</span>
                {/if}
            </div>
            {#if areaSummary}
                <p class="summary" class:warn={areaSummary.warn}>{areaSummary.title}</p>
            {:else}
                <p class="summary muted small">
                    Click regions on the map, search by name, or switch to “Draw box” for a custom
                    area.
                </p>
            {/if}
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">2</span>
                <h3>Map style</h3>
            </div>
            {#if presetError}
                <p class="small" style:color="var(--coral)">Couldn't load presets: {presetError}</p>
            {:else}
                <PresetCards {presets} />
            {/if}
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">3</span>
                <h3>Build</h3>
            </div>
            <BuildCard {selection} />
        </section>
    </div>
</div>

<style>
    .layout {
        display: grid;
        grid-template-columns: minmax(0, 1.3fr) minmax(330px, 1fr);
        gap: 14px;
        align-items: stretch;
    }

    .steps {
        display: flex;
        flex-direction: column;
        gap: 14px;
        min-width: 0;
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

    .summary {
        margin: 0;
        font-size: 14px;
    }

    .summary.warn {
        color: var(--coral);
    }

    @media (max-width: 940px) {
        .layout {
            grid-template-columns: 1fr;
        }

        :global(.map-wrap) {
            min-height: 380px;
        }
    }
</style>
