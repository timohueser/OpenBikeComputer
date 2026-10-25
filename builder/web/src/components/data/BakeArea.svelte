<script lang="ts">
    import { boundsOf, corners, extent, search, type Areas, type Bbox } from "../../lib/data/areas";
    import type { BakeRoute } from "../../lib/data/screens";
    import DataMap from "./DataMap.svelte";

    // One search over the saved areas and every Geofabrik area, or a box drawn on the map. The map
    // shows the chosen area, the one under the cursor in the list, and what the target serves.
    let {
        areas,
        chosen,
        bounds,
        served,
        choose,
    }: { areas: Areas | null; chosen: BakeRoute["area"]; bounds: Bbox | null; served: Bbox[]; choose: (area: BakeRoute["area"]) => void } = $props();

    let query = $state("");
    let active = $state(0);
    let drawing = $state(false);
    let drawn = $state<Bbox | null>(null);
    const found = $derived(areas ? search(query, areas) : []);
    const hovered = $derived(areas && found[active] ? boundsOf(found[active].id, areas) : null);
    const saved = $derived(areas?.regions ?? []);
    const cells = $derived([
        ...(hovered ? [{ bounds: corners(hovered), color: "var(--ink-faint)", label: found[active].name }] : []),
        ...(drawn ? [{ bounds: corners(drawn), color: "var(--running)", label: "the drawn box" }] : []),
        ...(bounds && !drawn ? [{ bounds: corners(bounds), color: "var(--running)", label: "the chosen area" }] : []),
    ]);
    const edge = (value: number) => +value.toFixed(4);

    function keys(event: KeyboardEvent) {
        if (event.key === "ArrowDown") active = Math.min(active + 1, found.length - 1);
        else if (event.key === "ArrowUp") active = Math.max(active - 1, 0);
        else if (event.key === "Enter" && found[active]) choose({ flag: "--region", value: found[active].id });
        else return;
        event.preventDefault();
    }
</script>

<div class="area">
    <div class="pick">
        <label class="search">
            <span>Country, region or saved area</span>
            <input
                type="search"
                role="combobox"
                aria-expanded={found.length > 0}
                aria-controls="area-results"
                aria-activedescendant={found[active] ? `area-${active}` : undefined}
                placeholder={areas ? "Switzerland, Bayern, central-switzerland…" : "Reading the areas…"}
                disabled={!areas}
                bind:value={query}
                oninput={() => (active = 0)}
                onkeydown={keys}
            />
        </label>
        {#if found.length}
            <ul id="area-results" role="listbox" aria-label="Areas">
                {#each found as hit, i (hit.id)}
                    <li id="area-{i}" role="option" aria-selected={i === active}>
                        <button type="button" onmouseenter={() => (active = i)} onclick={() => choose({ flag: "--region", value: hit.id })}>
                            <b>{hit.name}</b><span>{hit.detail}</span>
                        </button>
                    </li>
                {/each}
            </ul>
        {:else if query.trim() && areas}
            <p class="soft">No area has “{query.trim()}” in its name.</p>
        {:else if saved.length}
            <details class="saved">
                <summary>Saved areas ({saved.length})</summary>
                <div class="chips">
                    {#each saved as region (region.id)}
                        <button type="button" aria-pressed={chosen?.value === region.id} onclick={() => choose({ flag: "--region", value: region.id })}>{region.name}</button>
                    {/each}
                </div>
            </details>
        {/if}
        {#if areas && !areas.index}
            <p class="missing">Only saved areas: this machine has no Geofabrik index yet. In a terminal: <code>obc data fetch geofabrik-index</code></p>
        {/if}
        <div class="draw">
            <button type="button" aria-pressed={drawing} onclick={() => (drawing = !drawing)}>{drawing ? "Drawing: drag on the map" : "Draw a box"}</button>
            {#if drawn}
                <button type="button" class="primary" onclick={() => choose({ flag: "--box", value: [drawn!.west, drawn!.south, drawn!.east, drawn!.north].join(",") })}
                    >Use this box · {extent(drawn)}</button
                >
            {/if}
        </div>
    </div>
    <DataMap
        {cells}
        outline={served.map(corners)}
        {drawing}
        onbox={(south, west, north, east) => (drawn = { west: edge(west), south: edge(south), east: edge(east), north: edge(north) })}
    />
</div>

<style>
    .area {
        display: grid;
        grid-template-columns: minmax(260px, 360px) 1fr;
        gap: 16px;
        align-items: start;
    }
    .pick {
        display: flex;
        flex-direction: column;
        gap: 12px;
        min-width: 0;
    }
    .search {
        display: flex;
        flex-direction: column;
        gap: 6px;
        font-weight: 600;
    }
    input {
        min-height: 44px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        font: 400 15px var(--sans);
    }
    input:focus-visible,
    button:focus-visible {
        outline: 2px solid var(--running);
        outline-offset: 2px;
    }
    ul {
        display: flex;
        flex-direction: column;
        margin: 0;
        padding: 4px;
        list-style: none;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
    }
    li button {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        width: 100%;
        min-height: 44px;
        padding: 6px 10px;
        border: 0;
        text-align: left;
        font-weight: 400;
    }
    li[aria-selected="true"] button {
        background: var(--parchment);
    }
    li span {
        color: var(--ink-soft);
        font-size: 13px;
        line-height: 18px;
    }
    .saved summary {
        margin-bottom: 8px;
        color: var(--ink-soft);
        cursor: pointer;
    }
    .chips,
    .draw {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }
    .chips button {
        min-height: 36px;
        padding: 0 12px;
        font-weight: 500;
    }
    button[aria-pressed="true"] {
        background: var(--ink);
        color: var(--panel);
    }
    @media (max-width: 760px) {
        .area {
            grid-template-columns: 1fr;
        }
    }
</style>
