<script lang="ts">
    // Styles change presentation, never the selected geographic data.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import SkinEditor from "../skin/SkinEditor.svelte";
    import SkinPreviewThumbnail from "../skin/SkinPreviewThumbnail.svelte";
    import type { SkinEntry } from "../../lib/catalog/manifest";
    import { isCustomSkinId } from "../../lib/skin/custom";
    import { renderSkinPreviewFrames, type SkinPreviewFrame } from "../../lib/skin/preview";
    import Window from "../../lib/ui/Window.svelte";

    /** `plain`: each skin with its name and no note, for a window of the shell. */
    let { store, plain = false }: { store: CoverageStore; plain?: boolean } = $props();
    let previewUrls = $state<Record<string, string>>({});
    let customPreviewFrames = $state<Record<string, SkinPreviewFrame>>({});
    let editing = $state<SkinEntry | null>(null);
    /** The custom skin of the question before a delete. */
    let doomed = $state<SkinEntry | null>(null);
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
        // A catalog skin with no stored picture is drawn here, as a custom skin is.
        const custom = store.skins.filter((skin) => isCustomSkinId(skin.id) || !skin.preview);
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

</script>

<div class="theme-tabs" aria-label="Map style theme">
    <button type="button" aria-pressed={mode === "light"} class:active={mode === "light"} onclick={() => (mode = "light")}>Day</button>
    <button type="button" aria-pressed={mode === "dark"} class:active={mode === "dark"} onclick={() => (mode = "dark")}>Night</button>
</div>
{#if !plain}<p class="small faint theme-note">Light and dark styles are included.</p>{/if}
<div class="cards" class:plain>
    {#each visibleSkins as skin (skin.id)}
        <button
            type="button"
            class="skin"
            class:selected={selected.id === skin.id}
            aria-label={skin.name}
            aria-pressed={selected.id === skin.id}
            onclick={() => mode === "light" ? (store.lightSkinId = skin.id) : (store.darkSkinId = skin.id)}
        >
            {#if customPreviewFrames[skin.id]}
                <span class="shot">
                    <SkinPreviewThumbnail
                        frame={customPreviewFrames[skin.id]}
                        label={`${skin.name} rendered over Teningen`}
                    />
                </span>
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
            {#if plain || isCustomSkinId(skin.id)}
                <span class="label-line">
                    <span class="name">{skin.name}</span>
                    {#if isCustomSkinId(skin.id)}<span class="custom-tag">custom</span>{/if}
                </span>
            {/if}
        </button>
    {/each}
</div>
<div class="actions">
    <button type="button" class="action" onclick={() => (editing = selected)}>Edit the colours</button>
    {#if isCustomSkinId(selected.id)}
        <button type="button" class="action delete" onclick={() => (doomed = selected)}>Delete</button>
    {/if}
</div>

{#if editing}
    <SkinEditor {store} base={editing} theme={mode === "light" ? "Light" : "Dark"} onclose={() => (editing = null)} />
{/if}

{#if doomed}
    {@const id = doomed.id}
    <Window title={`Delete “${doomed.name}”?`} onClose={() => (doomed = null)}>
        {#snippet children(close)}
            <p>These colours leave this browser. A map on the device keeps them.</p>
            <div class="window-buttons">
                <button type="button" class="action" onclick={close}>Cancel</button>
                <button type="button" class="action danger" onclick={() => { close(); store.deleteCustomSkin(id); }}>Delete</button>
            </div>
        {/snippet}
    </Window>
{/if}

<style>
    .cards {
        display: flex;
        flex-wrap: wrap;
        gap: 10px;
    }

    .cards.plain {
        margin-top: 12px;
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
        width: min(100%, 144px);
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 9px;
        text-align: left;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        padding: 5px;
        transition:
            border-color 0.15s,
            box-shadow 0.15s;
    }

    .skin:hover {
        border-color: var(--wood);
    }

    .skin.selected {
        border: 2px solid var(--forest);
        padding: 4px;
    }

    .shot {
        display: block;
        width: 100%;
        aspect-ratio: 1;
        height: auto;
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

    /* Tells a style of the rider from a ready-made style with the same name. */
    .custom-tag {
        margin-right: 5px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }

    .actions {
        display: flex;
        align-items: center;
        gap: 8px;
        margin-top: 16px;
    }

    .action {
        min-height: 34px;
        padding: 5px 14px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }

    .action:hover {
        background: var(--parchment-2);
    }

    .danger {
        border-color: var(--coral);
        color: var(--coral);
    }

    /* Beside the edit button the delete is plain text at the far end; its window asks before it deletes. */
    .delete {
        margin-left: auto;
        border-color: transparent;
        color: var(--coral);
    }
</style>
