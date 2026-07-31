<script lang="ts">
    // Step 2 on the cell catalog: which skin the assembly is stamped with
    // (#1038; epic #1016 §4). A skin is ~2 KB of style table applied at
    // assembly time — the one fact worth a line here is that choosing one
    // never changes the download (mock R2·3's note), because riders coming
    // from the v1 flow are used to styles being separate artifacts.

    import type { CoverageStore } from "../../lib/coverage/store.svelte";

    let { store }: { store: CoverageStore } = $props();
</script>

<div class="cards">
    {#each store.catalog.skins as skin (skin.id)}
        <button
            type="button"
            class="skin"
            class:selected={store.skinId === skin.id}
            onclick={() => (store.skinId = skin.id)}
        >
            {#if skin.preview}
                <img src={skin.preview} alt="" class="shot" />
            {/if}
            <span class="name">{skin.name}</span>
            <span class="desc small muted">{skin.description}</span>
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
        gap: 5px;
        text-align: left;
        background: var(--parchment);
        border: 1px solid var(--parchment-3);
        border-radius: 12px;
        padding: 11px 12px;
        transition:
            border-color 0.15s,
            box-shadow 0.15s;
    }

    .skin:hover {
        border-color: var(--wood);
    }

    .skin.selected {
        border: 2px solid var(--forest);
        padding: 10px 11px;
        box-shadow: 0 2px 10px rgba(60, 107, 57, 0.16);
    }

    .shot {
        width: 100%;
        border-radius: 8px;
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
