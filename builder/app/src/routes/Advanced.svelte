<script lang="ts">
    import { onMount } from "svelte";
    import CategoryRail from "../components/advanced/CategoryRail.svelte";
    import LodTiers from "../components/advanced/LodTiers.svelte";
    import OutputTab from "../components/advanced/OutputTab.svelte";
    import ProfilesTab from "../components/advanced/ProfilesTab.svelte";
    import StyleTable from "../components/advanced/StyleTable.svelte";
    import { platform } from "../lib/platform";
    import { exportFile, importFile } from "../lib/config/edit";
    import { isBuildable, type Preset, type SchemaEnvelope } from "../lib/config/model";
    import { working } from "../lib/config/storage.svelte";
    import { confirmAction } from "../lib/ui/confirm.svelte";

    let tab = $state<"features" | "lods" | "routing" | "output">("features");
    let activeCat = $state("");
    let catalog = $state<{ keys: Record<string, string[]> }>({ keys: {} });
    let schema = $state<SchemaEnvelope | null>(null);
    let presets = $state<Preset[]>([]);
    let importError = $state<string | null>(null);
    let legacyConfig = $state<Record<string, unknown> | null>(null);
    let fileInput: HTMLInputElement;
    /** Where the last export landed, on a host that saves files rather than downloading them. */
    let exported = $state<string | null>(null);
    let exportError = $state<string | null>(null);

    // Read once: neither changes while the app runs. Both absent in a browser tab, where an
    // `<a download>` is the whole story and there is no file manager to point at.
    const saveText = platform.saveText;
    const revealFile = platform.revealFile;

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
        // Non-null wherever this route can load — `schema` is gated on
        // `caps.build || caps.styleEditor` and the editor is the latter.
        platform.schema?.().then((s) => (schema = s)).catch(() => (schema = null));
        platform.presets().then((p) => (presets = p)).catch(() => {});
        // The OSM tag-key catalog for the category rail — a static asset on
        // every host, and unrelated to the platform's `catalog()` (baked maps).
        fetch(`${import.meta.env.BASE_URL}osm_catalog.json`)
            .then((r) => (r.ok ? r.json() : { keys: {} }))
            .then((c) => (catalog = c?.keys ? c : { keys: {} }))
            .catch(() => {});
        // One-shot migration offer for pre-redesign server-side edits. Only the
        // dev host ever had a server-side config, hence the optional call.
        if (!localStorage.getItem("obcm.legacyPromptDismissed")) {
            platform
                .legacyConfig?.()
                .then((cfg) => (legacyConfig = cfg))
                .catch(() => {});
        }
    });

    $effect(() => {
        const cats = env ? Object.keys(env.config.features) : [];
        if (cats.length && !cats.includes(activeCat)) activeCat = cats[0];
    });

    async function resetToPreset() {
        const preset = presets.find((p) => p.id === env?.based_on?.id);
        // This editor only exists on a tier that serves config-carrying presets
        // (`caps.styleEditor`), so the guard is the type system's, not a case
        // that happens: there is nothing to reset to without a config.
        if (!preset || !isBuildable(preset)) return;
        // The explicit re-copy the envelope's semantics are built around: a preset never reaches a
        // working config except by someone asking for it. Asked through the app's own dialog,
        // because the browser's `confirm()` answers "no" on its own in the desktop webview — which
        // made this exact button a no-op there (see lib/ui/confirm.svelte.ts).
        const ok = await confirmAction({
            title: `Re-apply “${preset.name}” (v${preset.version})?`,
            body: "Your edits to this style are discarded and the preset is copied in again.",
            confirmLabel: "Reset to preset",
            destructive: true,
        });
        if (!ok) return;
        working.applyPreset(preset);
    }

    /**
     * Write the working config out as a plain, CLI-usable packer config.
     *
     * Two ways, because the two hosts disagree about what "save a file" is — not about the
     * document, which is byte-identical either way. A browser gets the anchor it has always had;
     * the desktop app gets a Rust write, because that anchor is silently inert inside a Tauri
     * webview (see `Platform.saveText`). Nothing here branches on a host name: the seam being
     * present *is* the statement that the fallback would not work.
     */
    async function exportNow() {
        if (!env) return;
        exported = null;
        exportError = null;
        const name = `obcm-style-${env.based_on?.id ?? "custom"}.json`;
        const text = exportFile(env);
        if (saveText) {
            try {
                exported = await saveText(name, text);
            } catch (e) {
                exportError = e instanceof Error ? e.message : String(e);
            }
            return;
        }
        const blob = new Blob([text], { type: "application/json" });
        const a = document.createElement("a");
        a.href = URL.createObjectURL(blob);
        a.download = name;
        a.click();
        URL.revokeObjectURL(a.href);
    }

    async function revealExport(path: string) {
        try {
            await revealFile?.(path);
        } catch (e) {
            exportError = e instanceof Error ? e.message : String(e);
        }
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
    <a href="#/" class="small">← Maps</a>
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

{#if exportError}
    <p class="error small">{exportError}</p>
{/if}

{#if exported}
    <p class="small muted saved">
        Saved to <span class="mono">{exported}</span>
        {#if revealFile}
            {@const path = exported}
            <button type="button" class="btn ghost" onclick={() => revealExport(path)}>Show</button>
        {/if}
    </p>
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
        <button type="button" class:active={tab === "routing"} onclick={() => (tab = "routing")}>
            Bike profiles
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
                        schemaRoot={schema?.schema ?? {}}
                        catalogValues={catalog.keys[activeCat] ?? []}
                        ondeleted={() => (activeCat = "")}
                    />
                {/key}
            {/if}
        </div>
    {:else if tab === "lods"}
        <LodTiers />
    {:else if tab === "routing"}
        <ProfilesTab {schema} />
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

    .saved {
        margin: 0 0 10px;
        display: flex;
        align-items: center;
        gap: 8px;
        flex-wrap: wrap;
        word-break: break-all;
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
