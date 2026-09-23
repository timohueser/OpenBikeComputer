<script lang="ts">
    // Step 2 on the cell catalog: the Light and Dark style tables stamped into
    // every map. Editing either never changes the downloaded cells.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import SkinEditor from "../skin/SkinEditor.svelte";
    import SkinPreviewThumbnail from "../skin/SkinPreviewThumbnail.svelte";
    import type { SkinEntry } from "../../lib/catalog/manifest";
    import { isCustomSkinId } from "../../lib/skin/custom";
    import { renderSkinPreviewFrames, type SkinPreviewFrame } from "../../lib/skin/preview";
    import { confirmAction } from "../../lib/ui/confirm.svelte";

    let { store }: { store: CoverageStore } = $props();
    let previewUrls = $state<Record<string, string>>({});
    let customPreviewFrames = $state<Record<string, SkinPreviewFrame>>({});
    let editing = $state<SkinEntry | null>(null);
    let mode = $state<"light" | "dark">("light");
    const visibleSkins = $derived(mode === "light" ? store.lightSkins : store.darkSkins);
    const selected = $derived(mode === "light" ? store.lightSkin : store.darkSkin);

    $effect(() => {
        let live = true;
        const objectUrls: string[] = [];
        for (const skin of store.catalog.skins) {
            void store.client
                .skinPreview(skin.id)
                .then((bytes) => {
                    if (!bytes || !live) return;
                    // Copy into an ordinary ArrayBuffer-backed view for Blob's
                    // cross-runtime type and retain the URL for this component only.
                    const copy = Uint8Array.from(bytes);
                    const url = URL.createObjectURL(new Blob([copy], { type: "image/png" }));
                    objectUrls.push(url);
                    previewUrls = { ...previewUrls, [skin.id]: url };
                })
                .catch(() => {
                    // A failed pin is a refusal, not a reason to lose the picker:
                    // keep the neutral placeholder and let the skin remain usable.
                });
        }
        return () => {
            live = false;
            for (const url of objectUrls) URL.revokeObjectURL(url);
        };
    });

    $effect(() => {
        const custom = store.skins.filter((skin) => isCustomSkinId(skin.id));
        const controller = new AbortController();
        let live = true;
        if (custom.length === 0) {
            customPreviewFrames = {};
        } else {
            void renderSkinPreviewFrames(store.rootBody, custom, { signal: controller.signal })
                .then((frames) => {
                    if (live) customPreviewFrames = frames;
                })
                .catch(() => {
                    // The validated skin remains selectable if wasm or the
                    // fixture cannot load; its card falls back to the neutral shot.
                    if (live) customPreviewFrames = {};
                });
        }
        return () => {
            live = false;
            controller.abort();
        };
    });

    async function removeSelected() {
        if (!isCustomSkinId(selected.id)) return;
        const doomed = selected;
        const ok = await confirmAction({
            title: `Delete “${doomed.name}”?`,
            body: "This removes the custom style from this browser. Hosted styles and downloaded maps are unchanged.",
            confirmLabel: "Delete style",
            destructive: true,
        });
        if (ok) store.deleteCustomSkin(doomed.id);
    }
</script>

<div class="theme-tabs" aria-label="Map style theme">
    <button type="button" aria-pressed={mode === "light"} class:active={mode === "light"} onclick={() => (mode = "light")}>Light</button>
    <button type="button" aria-pressed={mode === "dark"} class:active={mode === "dark"} onclick={() => (mode = "dark")}>Dark</button>
</div>
<p class="small faint theme-note">Both styles are included. The device follows its display theme.</p>
<div class="cards">
    {#each visibleSkins as skin (skin.id)}
        <button
            type="button"
            class="skin"
            class:selected={selected.id === skin.id}
            onclick={() => mode === "light" ? (store.lightSkinId = skin.id) : (store.darkSkinId = skin.id)}
        >
            {#if isCustomSkinId(skin.id)}
                {#if customPreviewFrames[skin.id]}
                    <span class="shot">
                        <SkinPreviewThumbnail
                            frame={customPreviewFrames[skin.id]}
                            label={`${skin.name} rendered over Teningen`}
                        />
                    </span>
                {:else}
                    <span class="shot placeholder" aria-hidden="true"></span>
                {/if}
            {:else if previewUrls[skin.id]}
                <img
                    class="shot"
                    src={previewUrls[skin.id]}
                    alt={`${skin.name} rendered over Teningen`}
                    width="240"
                    height="240"
                    loading="lazy"
                />
            {:else}
                <span class="shot placeholder" aria-hidden="true"></span>
            {/if}
            <span class="label-line">
                <span class="name">{skin.name}</span>
                {#if isCustomSkinId(skin.id)}<span class="custom-tag">custom</span>{/if}
            </span>
        </button>
    {/each}
</div>
<div class="actions">
    <p class="small faint note">Styles reuse the same cells — edits never re-download anything.</p>
    {#if isCustomSkinId(selected.id)}
        <button type="button" class="text-action danger" onclick={removeSelected}>Delete</button>
    {/if}
    <button type="button" class="btn ghost customize" onclick={() => (editing = selected)}>
        {isCustomSkinId(selected.id) ? `Edit ${mode}` : `Customize ${mode}`}
    </button>
</div>

{#if editing}
    <SkinEditor {store} base={editing} theme={mode === "light" ? "Light" : "Dark"} onclose={() => (editing = null)} />
{/if}

<style>
    .cards {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(168px, 1fr));
        gap: 10px;
    }

    .theme-tabs {
        display: inline-flex;
        padding: 3px;
        border: 1px solid var(--parchment-3);
        border-radius: 9px;
        background: var(--parchment-2);
    }

    .theme-tabs button {
        min-width: 72px;
        padding: 6px 12px;
        border: 0;
        border-radius: 6px;
        background: transparent;
        color: var(--ink-faint);
        font-weight: 600;
    }

    .theme-tabs button.active {
        color: var(--ink);
        background: var(--panel);
        box-shadow: 0 1px 4px rgba(27, 36, 23, 0.12);
    }

    .theme-note {
        margin: 7px 0 10px;
    }

    .skin {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 9px;
        text-align: left;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        padding: 7px 7px 11px;
        transition:
            border-color 0.15s,
            box-shadow 0.15s;
    }

    .skin:hover {
        border-color: var(--wood);
    }

    .skin.selected {
        border: 2px solid var(--forest);
        padding: 6px 6px 10px;
        box-shadow: 0 2px 10px rgba(60, 107, 57, 0.16);
    }

    .shot {
        display: block;
        width: 100%;
        aspect-ratio: 1;
        object-fit: cover;
        border-radius: 8px;
        border: 1px solid var(--parchment-3);
        background: var(--parchment-2);
    }

    .placeholder {
        background: linear-gradient(135deg, var(--parchment-2), var(--parchment-3));
    }

    .shot {
        overflow: hidden;
    }

    .label-line {
        display: flex;
        align-items: center;
        justify-content: space-between;
        width: 100%;
        gap: 6px;
    }

    .name {
        font-weight: 600;
        font-size: 14px;
        color: var(--ink);
        padding: 0 5px;
    }

    .custom-tag {
        margin-right: 5px;
        padding: 2px 5px;
        color: var(--forest);
        background: rgba(60, 107, 57, 0.1);
        border-radius: 5px;
        font-size: 10px;
        font-weight: 700;
        letter-spacing: 0.06em;
        text-transform: uppercase;
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-top: 8px;
    }

    .note {
        flex: 1;
        margin: 0;
    }

    .customize {
        padding: 7px 12px;
    }

    .text-action {
        padding: 5px;
        border: 0;
        background: transparent;
        color: var(--ink-faint);
    }

    .text-action.danger:hover {
        color: var(--coral);
    }
</style>
