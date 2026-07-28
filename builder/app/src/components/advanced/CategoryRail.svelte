<script lang="ts">
    import { working } from "../../lib/config/storage.svelte";

    let {
        active,
        catalogKeys,
        onselect,
    }: {
        active: string;
        catalogKeys: string[];
        onselect: (cat: string) => void;
    } = $props();

    let adding = $state(false);
    let newKey = $state("");
    let dupe = $state(false);

    const cats = $derived(
        working.envelope ? Object.keys(working.envelope.config.features) : [],
    );

    function count(cat: string): number {
        return Object.keys(working.envelope?.config.features[cat] ?? {}).length;
    }

    function commit() {
        const key = newKey.trim();
        if (!key) {
            adding = false;
            return;
        }
        if (working.envelope!.config.features[key]) {
            dupe = true;
            return;
        }
        working.envelope!.config.features[key] = {};
        working.markModified();
        adding = false;
        newKey = "";
        onselect(key);
    }
</script>

<nav>
    {#each cats as cat (cat)}
        <button type="button" class:active={cat === active} onclick={() => onselect(cat)}>
            <span class="mono">{cat}</span>
            <span class="faint">{count(cat)}</span>
        </button>
    {/each}

    {#if adding}
        <input
            type="text"
            class:dupe
            list="osm-keys"
            placeholder="OSM tag key (e.g. railway)"
            bind:value={newKey}
            oninput={() => (dupe = false)}
            onkeydown={(e) => {
                if (e.key === "Enter") commit();
                else if (e.key === "Escape") (adding = false), (newKey = "");
            }}
            onblur={commit}
        />
        <datalist id="osm-keys">
            {#each catalogKeys.filter((k) => !cats.includes(k)) as k (k)}
                <option value={k}></option>
            {/each}
        </datalist>
    {:else}
        <button type="button" class="add" onclick={() => (adding = true)}>+ add category</button>
    {/if}
</nav>

<style>
    nav {
        display: flex;
        flex-direction: column;
        gap: 2px;
        min-width: 148px;
    }

    nav > button {
        display: flex;
        justify-content: space-between;
        gap: 10px;
        background: none;
        border: none;
        border-radius: 7px;
        padding: 6px 10px;
        font-size: 13px;
        color: var(--ink-soft);
        text-align: left;
    }

    nav > button:hover {
        background: rgba(95, 125, 61, 0.1);
    }

    nav > button.active {
        background: var(--parchment-2);
        color: var(--ink);
        font-weight: 600;
    }

    .add {
        color: var(--forest);
    }

    input {
        font-size: 12.5px;
        padding: 5px 8px;
    }

    input.dupe {
        border-color: var(--coral);
    }
</style>
