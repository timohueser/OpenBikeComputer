<script lang="ts">
    import { onMount } from "svelte";
    import BuildCard from "../components/BuildCard.svelte";
    import DeviceStep from "../components/device/DeviceStep.svelte";
    import MapSendStep from "../components/device/MapSendStep.svelte";
    import Gated from "../components/Gated.svelte";
    import MapPanel from "../components/MapPanel.svelte";
    import PresetCards from "../components/PresetCards.svelte";
    import StorageCard from "../components/StorageCard.svelte";
    import DownloadStep from "../components/catalog/DownloadStep.svelte";
    import PresetStep from "../components/catalog/PresetStep.svelte";
    import RegionStep from "../components/catalog/RegionStep.svelte";
    import { regionState } from "../lib/catalog/availability";
    import { artifactFilename } from "../lib/catalog/download";
    import { catalogStore } from "../lib/catalog/store.svelte";
    import { platform } from "../lib/platform";
    import { available } from "../lib/platform/gating";
    import { isBuildable, type Preset } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";
    import type { CatalogHints } from "../lib/map/regionPicker";
    import type { AreaSelection } from "../lib/map/selection";

    let { active = true }: { active?: boolean } = $props();

    // Non-null exactly on a host with `caps.build`. Handed to `<Gated>`, which
    // makes the one check and passes the narrowed value to the card — so step 3
    // is present either way, live or dead-with-a-reason.
    const buildMap = platform.buildMap;

    // No packer means the maps come pre-baked, which is the same fact from the
    // other side: the catalog is the only place a map can come from, one baked
    // artifact at a time. That is the gate — a capability, never a host name.
    const catalogMode = buildMap === null;

    // Present only on a host that puts gigabytes on someone's disk. Not gated
    // and not numbered: there is nothing to tell a web visitor about caches they
    // do not have, so the card is simply absent rather than disabled (#901's own
    // rule — show nothing where there is no moment of intent to gate).
    const storage = platform.storage;

    let presets = $state<Preset[]>([]);
    let presetError = $state<string | null>(null);
    let selection = $state<AreaSelection | null>(null);
    let mapPanel = $state<{ removeRegion: (id: string) => void; selectRegion: (id: string) => void }>();

    const PRESET_KEY = "obcm.catalogPreset";
    let presetId = $state<string | null>(null);

    onMount(async () => {
        if (catalogMode) {
            // The catalog failing is one failure, reported once, in step 1 —
            // the styles come out of the same document, so a second copy of the
            // same sentence in step 2 would just be noise.
            await catalogStore.load();
            presets = catalogStore.presets;
            const remembered = localStorage.getItem(PRESET_KEY);
            presetId =
                (remembered && presets.some((p) => p.id === remembered) ? remembered : null) ??
                presets[0]?.id ??
                null;
            return;
        }
        const restored = working.restore();
        try {
            presets = await platform.presets();
            // First visit: the default preset is the working config.
            const first = presets.find(isBuildable);
            if (!restored && first) working.applyPreset(first);
        } catch (e) {
            presetError = e instanceof Error ? e.message : String(e);
        }
    });

    function pickPreset(id: string) {
        presetId = id;
        try {
            localStorage.setItem(PRESET_KEY, id);
        } catch {
            // non-fatal
        }
    }

    // --- the catalog picker's derived state -------------------------------

    const index = $derived(catalogStore.index);
    const device = $derived(catalogStore.device);
    /** Single-select on this tier, so the selection is one region or none. */
    const entry = $derived.by(() => {
        const id = selection?.mode === "regions" ? selection.regionIds[0] : undefined;
        return id && index ? (index.get(id) ?? null) : null;
    });
    const artifact = $derived(
        entry && presetId ? (entry.artifacts.find((a) => a.preset_id === presetId) ?? null) : null,
    );
    // What step 4 sends. `null` where no baked artifact is selected — including
    // on a tier with a packer, where step 3 builds one and there is no catalog
    // at all; the device step then offers the file path alone.
    const deviceArtifact = $derived(
        artifact
            ? {
                  filename: artifactFilename(artifact),
                  url: artifact.url,
                  bytes: artifact.bytes,
                  sha256: artifact.sha256,
              }
            : null,
    );

    const hints = $derived.by<CatalogHints | null>(() => {
        const idx = index;
        if (!idx) return null;
        return {
            coverageIds: idx.bakedIds,
            tone: (id) => {
                const region = idx.get(id);
                return region ? regionState(region, device) : "not-baked";
            },
        };
    });

    const bboxSummary = $derived.by(() => {
        if (!selection || selection.mode !== "bbox" || !selection.bbox) return null;
        const [w, s, e, n] = selection.bbox;
        const huge = (selection.areaKm2Raw ?? 0) > 500_000;
        return {
            title: `Box W ${w.toFixed(3)} · S ${s.toFixed(3)} · E ${e.toFixed(3)} · N ${n.toFixed(3)}`,
            hint: selection.coveringNames.length
                ? `≈ ${selection.areaKm2} · from ${selection.coveringNames.join(", ")}` +
                  (huge ? " · large area — expect a long download and build" : "")
                : "no downloadable region covers this area",
            warn: selection.coveringNames.length === 0 || huge,
        };
    });

    const regionCount = $derived(
        selection?.mode === "regions" ? selection.regionIds.length : 0,
    );

    const areaHint = $derived(
        selection?.mode === "bbox"
            ? (bboxSummary?.hint ?? null)
            : regionCount
              ? `${regionCount} ${regionCount === 1 ? "region" : "regions"}`
              : null,
    );
</script>

<div class="layout">
    <MapPanel
        {active}
        {hints}
        singleSelect={catalogMode}
        bind:this={mapPanel}
        onchange={(sel) => (selection = sel)}
    />

    <div class="steps">
        {#if catalogMode && catalogStore.staleReason}
            <p class="notice small">
                Showing the catalog cached {new Date(catalogStore.cachedAt ?? 0).toISOString().slice(0, 10)}
                — the published one was refused: {catalogStore.staleReason}
            </p>
        {/if}

        <section class="card">
            <div class="step-head">
                <span class="num">1</span>
                <h3>Area</h3>
                {#if areaHint && !catalogMode}
                    <span class="small faint">{areaHint}</span>
                {/if}
            </div>
            {#if catalogMode}
                {#if catalogStore.state === "error"}
                    <p class="small" style:color="var(--coral)">{catalogStore.error}</p>
                {:else if index}
                    <RegionStep
                        {index}
                        {entry}
                        {artifact}
                        {device}
                        onselect={(id) => mapPanel?.selectRegion(id)}
                    />
                {:else}
                    <p class="summary muted small">Loading the map catalog…</p>
                {/if}
            {:else if selection?.mode === "regions" && regionCount}
                <div class="region-chips">
                    {#each selection.regionIds as id, i (id)}
                        <span class="chip">
                            {selection.regionNames[i]}
                            <button
                                type="button"
                                title="Remove {selection.regionNames[i]}"
                                aria-label="Remove {selection.regionNames[i]}"
                                onclick={() => mapPanel?.removeRegion(id)}>×</button
                            >
                        </span>
                    {/each}
                </div>
            {:else if bboxSummary}
                <p class="summary" class:warn={bboxSummary.warn}>{bboxSummary.title}</p>
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
            {:else if catalogMode}
                {#if presets.length}
                    <PresetStep {presets} {entry} {presetId} {device} onpick={pickPreset} />
                {:else}
                    <p class="summary muted small">No styles to show — the catalog didn't load.</p>
                {/if}
            {:else}
                <PresetCards presets={presets.filter(isBuildable)} />
            {/if}
        </section>

        <!-- One step, two ways to end up with a map. Where there is a packer,
             step 3 builds one; where there isn't, the catalog hands one over —
             and the build stays on screen underneath, dead and with its reason,
             because "you can't cut your own map here" is the thing worth
             discovering at exactly this moment (#901). Deliberately not a
             fourth numbered step: C3 (#902) owns the next number. -->
        <section class="card">
            <div class="step-head">
                <span class="num">3</span>
                <h3>{catalogMode ? "Download" : "Build"}</h3>
            </div>
            {#if catalogMode}
                <DownloadStep
                    {entry}
                    {artifact}
                    {device}
                    preset={presets.find((p) => p.id === presetId) ?? null}
                />
            {/if}
            <Gated need="build" value={buildMap}>
                {#snippet children(start)}
                    <BuildCard {selection} buildMap={start} />
                {/snippet}
                {#snippet unavailable(reason)}
                    <button type="button" class="btn primary" disabled aria-describedby={reason}>
                        Build map
                    </button>
                {/snippet}
            </Gated>
        </section>

        <section class="card">
            <div class="step-head">
                <span class="num">4</span>
                <h3>{available("deviceDashboard") ? "Send to device" : "Device"}</h3>
            </div>
            <!-- The selected region's artifact, reduced to the four facts a
                 device write turns on. `lib/device/` never imports the catalog:
                 a `.obcm` the rider already has — a desktop build, an older
                 download — has no manifest behind it and takes the same path.

                 Two shapes for the same step: where the device has a page of
                 its own (and the header the chip), only the map leaves from
                 here; everywhere else the full device step stays. -->
            {#if available("deviceDashboard")}
                <MapSendStep artifact={deviceArtifact} />
            {:else}
                <DeviceStep artifact={deviceArtifact} />
            {/if}
        </section>

        <!-- Unnumbered, and after the steps: this is not part of making a map,
             it is what making maps has left on the disk. -->
        {#if storage}
            <StorageCard {storage} />
        {/if}
    </div>
</div>

<style>
    .layout {
        flex: 1; /* fills main's column so the map absorbs tall screens */
        min-height: 0; /* …and never grows past them: the column scrolls instead */
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
        /* breathing room so the scrollbar doesn't sit on the cards */
        padding-right: 4px;
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

    .region-chips {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }

    .summary.warn {
        color: var(--coral);
    }

    .notice {
        margin: 0;
        padding: 8px 12px;
        border-radius: 11px;
        background: rgba(227, 173, 51, 0.18);
        border: 1px solid var(--amber);
        color: var(--ink);
        line-height: 1.4;
    }

    @media (max-width: 940px) {
        .layout {
            grid-template-columns: 1fr;
        }

        .steps {
            overflow: visible;
            padding-right: 0;
        }

        :global(.map-wrap) {
            min-height: 380px;
        }
    }
</style>
