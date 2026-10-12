<script lang="ts">
    import { onMount } from "svelte";
    import { command } from "../../lib/data/cli";
    import { corners, extract, outline, type Areas, type Bbox } from "../../lib/data/areas";
    import { CliQuery } from "../../lib/data/query.svelte";
    import DataMap from "./DataMap.svelte";

    // The area of the next review: a preset, Geofabrik areas, or a box. `apply` gives the flag.
    let { region, apply }: { region: string; apply: (flag: "--region" | "--box", value: string) => void } = $props();

    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => void areas.refresh());
    let tab = $state<"preset" | "geofabrik" | "box">("preset");
    let query = $state("");
    let ticked = $state<string[]>([]);
    let box = $state<Bbox>({ west: NaN, south: NaN, east: NaN, north: NaN });
    const found = $derived.by(() => {
        const words = query.trim().toLowerCase();
        const all = areas.output?.areas ?? [];
        return words ? all.filter((area) => area.id.includes(words) || area.name.toLowerCase().includes(words)).slice(0, 40) : [];
    });
    const drawn = $derived(Object.values(box).every(Number.isFinite));
    const reads = $derived(drawn ? extract(areas.output, box) : null);
    const cells = $derived([
        ...ticked.flatMap((id) => outline(areas.output, id)).map((bounds) => ({ bounds: corners(bounds), color: "var(--running)", label: "ticked" })),
        ...(drawn ? [{ bounds: corners(box), color: "var(--running)", label: "box" }] : []),
    ]);
    const regions = $derived(found.map(({ id, name, bounds }) => ({ id, name, rings: [rectangle(bounds)] })));
    function rectangle({ west, south, east, north }: Bbox) {
        return [[south, west], [south, east], [north, east], [north, west], [south, west]] as [number, number][];
    }
    const toggle = (id: string) => (ticked = ticked.includes(id) ? ticked.filter((at) => at !== id) : [...ticked, id]);
    const edge = (value: number) => +value.toFixed(4);
</script>

<div class="tabs" role="tablist" aria-label="Area">
    {#each [["preset", "Preset"], ["geofabrik", "Geofabrik area"], ["box", "Draw a box"]] as [id, label] (id)}
        <button type="button" role="tab" aria-selected={tab === id} onclick={() => (tab = id as typeof tab)}>{label}</button>
    {/each}
</div>

{#if tab === "preset"}
    <div class="title">
        {#each areas.output?.regions ?? [] as preset (preset.id)}
            <button type="button" aria-pressed={preset.id === region} onclick={() => apply("--region", preset.id)}>{preset.name}</button>
        {/each}
    </div>
{:else if tab === "geofabrik"}
    {#if areas.output && !areas.output.index}
        <p class="missing">The store has no Geofabrik index. In a terminal: <code>{command(["fetch", "geofabrik-index"])}</code></p>
    {:else}
        <label class="title">Search <input bind:value={query} placeholder="baden-wuerttemberg" /></label>
        <div class="found">
            {#each found as area (area.id)}
                <label><input type="checkbox" checked={ticked.includes(area.id)} onchange={() => toggle(area.id)} /> {area.name} <code>{area.id}</code></label>
            {/each}
        </div>
        <button type="button" disabled={!ticked.length} onclick={() => apply("--region", ticked.join(","))}
            >Review {ticked.length} {ticked.length === 1 ? "area" : "areas"}</button
        >
    {/if}
{:else}
    <p class="soft">Drag on the map to draw a box, or type its edges in degrees.</p>
    <form class="title" onsubmit={(event) => (event.preventDefault(), apply("--box", [box.west, box.south, box.east, box.north].join(",")))}>
        {#each ["west", "south", "east", "north"] as const as side (side)}
            <label>{side[0].toUpperCase() + side.slice(1)} <input type="number" step="any" required bind:value={box[side]} /></label>
        {/each}
        <button type="submit">Review this box</button>
    </form>
    {#if reads}<p class="soft">It reads the extract {reads}.</p>{/if}
{/if}

<DataMap
    {cells}
    outline={outline(areas.output, region).map(corners)}
    regions={tab === "geofabrik" ? regions : []}
    picking={tab === "geofabrik"}
    drawing={tab === "box"}
    onpick={toggle}
    onbox={(south, west, north, east) => (box = { west: edge(west), south: edge(south), east: edge(east), north: edge(north) })}
/>

<style>
    .tabs {
        display: flex;
        flex-wrap: wrap;
        border-bottom: 1px solid var(--line);
    }
    .tabs button {
        padding: 0 12px;
        overflow-wrap: normal;
        border: 0;
        border-radius: 0;
        border-bottom: 3px solid transparent;
        background: none;
    }
    .tabs button[aria-selected="true"] {
        border-bottom-color: var(--ink);
    }
    button[aria-pressed="true"] {
        background: var(--ink);
        color: var(--panel);
    }
    .found {
        display: flex;
        flex-direction: column;
        max-height: 200px;
        overflow: auto;
    }
    input:not([type="checkbox"]) {
        width: 9em;
        min-height: 40px;
        margin-left: 6px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font: inherit;
    }
</style>
