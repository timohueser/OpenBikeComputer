<script lang="ts">
    // Step 2 on the cell catalog: which skin the assembly is stamped with
    // (#1038; epic #1016 §4). A skin is ~2 KB of style table applied at
    // assembly time — the one fact worth a line here is that choosing one
    // never changes the download (mock R2·3's note), because riders coming
    // while leaving the downloaded cells unchanged.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";

    let { store }: { store: CoverageStore } = $props();
    let previewUrls = $state<Record<string, string>>({});

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
</script>

<div class="cards">
    {#each store.catalog.skins as skin (skin.id)}
        <button
            type="button"
            class="skin"
            class:selected={store.skinId === skin.id}
            onclick={() => (store.skinId = skin.id)}
        >
            {#if previewUrls[skin.id]}
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
            <span class="copy">
                <span class="name">{skin.name}</span>
                <span class="desc small muted">{skin.description}</span>
            </span>
        </button>
    {/each}
</div>
<p class="small faint note">
    Skins restyle the same cells — changing one never re-downloads anything.
</p>

<style>
    .cards {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(168px, 1fr));
        gap: 10px;
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

    .copy {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 5px;
        padding: 0 5px;
    }

    .name {
        font-weight: 600;
        font-size: 14px;
        color: var(--ink);
    }

    .desc {
        line-height: 1.35;
    }

    .note {
        margin: 8px 0 0;
    }
</style>
