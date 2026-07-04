<script lang="ts">
    import { onMount } from "svelte";
    import CategoryRail from "../components/advanced/CategoryRail.svelte";
    import LodTiers from "../components/advanced/LodTiers.svelte";
    import OutputTab from "../components/advanced/OutputTab.svelte";
    import StyleTable from "../components/advanced/StyleTable.svelte";
    import { api } from "../lib/api/client";
    import { API_BASE } from "../lib/constants";
    import { exportFile, importFile } from "../lib/config/edit";
    import type { Preset, SchemaEnvelope } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";

    let tab = $state<"features" | "lods" | "output">("features");
    let activeCat = $state("");
    let catalog = $state<{ keys: Record<string, string[]> }>({ keys: {} });
    let schema = $state<SchemaEnvelope | null>(null);
    let presets = $state<Preset[]>([]);
    let importError = $state<string | null>(null);
    let legacyConfig = $state<Record<string, unknown> | null>(null);
    let fileInput: HTMLInputElement;

    const env = $derived(working.envelope);
    const basedOnName = $derived(
        presets.find((p) => p.id === env?.based_on?.id)?.name ?? env?.based_on?.id ?? null,
    );

    // Fields the table renders bespoke columns for; everything else the schema
    // declares becomes an extra column via SchemaField (v6 line_style/color2).
    const BESPOKE = new Set(["color", "z_index", "weight", "priority", "min_lod"]);
    const extras = $derived(
        schema
            ? Object.entries(schema.schema.$defs.style.properties).filter(([k]) => !BESPOKE.has(k))
            : ([] as [string, unknown][]),
    );

    onMount(async () => {
        if (!working.envelope) working.restore();
        api.schema().then((s) => (schema = s)).catch(() => (schema = null));
        api.presets().then((p) => (presets = p)).catch(() => {});
        fetch(`${import.meta.env.BASE_URL}osm_catalog.json`)
            .then((r) => (r.ok ? r.json() : { keys: {} }))
            .then((c) => (catalog = c?.keys ? c : { keys: {} }))
            .catch(() => {});
        // One-shot migration offer for pre-redesign server-side edits.
        if (!localStorage.getItem("obcm.legacyPromptDismissed")) {
            fetch(`${API_BASE}/config/legacy`)
                .then((r) => (r.ok ? r.json() : null))
                .then((cfg) => (legacyConfig = cfg))
                .catch(() => {});
        }
    });

    $effect(() => {
        const cats = env ? Object.keys(env.config.features) : [];
        if (cats.length && !cats.includes(activeCat)) activeCat = cats[0];
    });

    function resetToPreset() {
        const preset = presets.find((p) => p.id === env?.based_on?.id);
        if (!preset) return;
        if (!confirm(`Discard your edits and re-apply "${preset.name}" (v${preset.version})?`)) return;
        working.applyPreset(preset);
    }

    function exportNow() {
        if (!env) return;
        const blob = new Blob([exportFile(env)], { type: "application/json" });
        const a = document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = `obcm-style-${env.based_on?.id ?? "custom"}.json`;
        a.click();
        URL.revokeObjectURL(a.href);
    }

    async function importPicked(files: FileList | null) {
        importError = null;
        const file = files?.[0];
        if (!file) return;
        const imported = importFile(await file.text());
        if (!imported) {
            importError = `${file.name} is not a recognizable config or stylesheet.`;
            return;
        }
        working.adopt(imported);
    }

    function importLegacy() {
        if (!legacyConfig) return;
        const imported = importFile(JSON.stringify(legacyConfig));
        if (imported) working.adopt(imported);
        dismissLegacy();
    }

    function dismissLegacy() {
        legacyConfig = null;
        localStorage.setItem("obcm.legacyPromptDismissed", "1");
    }
</script>

<div class="head">
    <a href="#/" class="small">← Map builder</a>
    <h2>Advanced editor</h2>
    {#if env}
        <span class="badge small">
            {#if !env.modified}Preset: {basedOnName}
            {:else if basedOnName}Custom — based on {basedOnName}
            {:else}Custom{/if}
        </span>
    {/if}
    <span class="actions">
        {#if env?.modified && env?.based_on}
            <button type="button" class="btn ghost" onclick={resetToPreset}>Reset to preset</button>
        {/if}
        <button type="button" class="btn ghost" onclick={exportNow} disabled={!env}>Export</button>
        <button type="button" class="btn ghost" onclick={() => fileInput.click()}>Import</button>
        <input
            type="file"
            accept=".json,application/json"
            hidden
            bind:this={fileInput}
            onchange={(e) => {
                importPicked(e.currentTarget.files);
                e.currentTarget.value = "";
            }}
        />
    </span>
</div>

{#if importError}
    <p class="error small">{importError}</p>
{/if}

{#if legacyConfig}
    <div class="legacy card">
        <span class="small">
            Found edits from the previous editor (<span class="mono">user_config.json</span>).
            Import them as your working config?
        </span>
        <span class="legacy-actions">
            <button type="button" class="btn ghost" onclick={importLegacy}>Import</button>
            <button type="button" class="btn ghost" onclick={dismissLegacy}>Dismiss</button>
        </span>
    </div>
{/if}

{#if !env}
    <div class="card">
        <p>No working config yet — pick a map style on the <a href="#/">main page</a> first.</p>
    </div>
{:else}
    <div class="tabs">
        <button type="button" class:active={tab === "features"} onclick={() => (tab = "features")}>
            Features &amp; styling
        </button>
        <button type="button" class:active={tab === "lods"} onclick={() => (tab = "lods")}>
            Detail levels
        </button>
        <button type="button" class:active={tab === "output"} onclick={() => (tab = "output")}>
            Output
        </button>
    </div>

    {#if tab === "features"}
        <div class="features">
            <CategoryRail
                active={activeCat}
                catalogKeys={Object.keys(catalog.keys)}
                onselect={(c) => (activeCat = c)}
            />
            {#if activeCat}
                {#key activeCat}
                    <StyleTable
                        cat={activeCat}
                        extras={extras as [string, Record<string, unknown>][]}
                        catalogValues={catalog.keys[activeCat] ?? []}
                        ondeleted={() => (activeCat = "")}
                    />
                {/key}
            {/if}
        </div>
    {:else if tab === "lods"}
        <LodTiers />
    {:else}
        <OutputTab {schema} />
    {/if}
{/if}

<style>
    .head {
        display: flex;
        align-items: center;
        gap: 14px;
        margin-bottom: 12px;
        flex-wrap: wrap;
    }

    .head h2 {
        font-size: 22px;
    }

    .badge {
        background: rgba(227, 173, 51, 0.28);
        border: 1px solid var(--amber);
        border-radius: 999px;
        padding: 2px 10px;
        font-weight: 600;
    }

    .actions {
        margin-left: auto;
        display: flex;
        gap: 8px;
    }

    .error {
        color: var(--coral);
        margin: 0 0 10px;
    }

    .legacy {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 14px;
        margin-bottom: 12px;
        border-left: 4px solid var(--amber);
        border-radius: 0 16px 16px 0;
    }

    .legacy-actions {
        display: flex;
        gap: 8px;
        flex: none;
    }

    .tabs {
        display: flex;
        gap: 18px;
        border-bottom: 1px solid var(--line-strong);
        margin-bottom: 14px;
    }

    .tabs button {
        background: none;
        border: none;
        padding: 6px 2px 9px;
        font-size: 14px;
        color: var(--ink-soft);
        border-bottom: 2px solid transparent;
        margin-bottom: -1px;
    }

    .tabs button.active {
        color: var(--ink);
        font-weight: 600;
        border-bottom-color: var(--forest);
    }

    .features {
        display: flex;
        gap: 14px;
        align-items: flex-start;
    }

    @media (max-width: 800px) {
        .features {
            flex-direction: column;
        }
    }
</style>
