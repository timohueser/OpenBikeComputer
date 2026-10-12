<script lang="ts">
    import { onMount } from "svelte";
    import { corners, type Areas, type Bbox } from "../../lib/data/areas";
    import { command } from "../../lib/data/cli";
    import { CliQuery } from "../../lib/data/query.svelte";
    import DataMap from "./DataMap.svelte";
    import Failure from "./Failure.svelte";

    // The area of the next review: a preset, Geofabrik areas, or a box. `apply` gives the flag. The
    // map shows `boxes`, the area that the target serves, and the choice of the open tab only.
    let { region, boxes, apply }: { region: string; boxes: Bbox[]; apply: (flag: "--region" | "--box", value: string) => void } =
        $props();

    const areas = new CliQuery<Areas>(["areas"]);
    onMount(() => void areas.refresh());
    let tab = $state<"preset" | "geofabrik" | "box">("preset");
    let query = $state("");
    let ticked = $state<string[]>([]);
    let box = $state<Bbox>({ west: NaN, south: NaN, east: NaN, north: NaN });
    const all = $derived(areas.output?.areas ?? []);
    const found = $derived.by(() => {
        const words = query.trim().toLowerCase();
        return words ? all.filter((area) => area.id.includes(words) || area.name.toLowerCase().includes(words)).slice(0, 40) : [];
    });
    const drawn = $derived(Object.values(box).every(Number.isFinite));
    const chosen = $derived.by(() => {
        if (tab === "geofabrik") return all.filter((area) => ticked.includes(area.id)).map((area) => area.bounds);
        return tab === "box" && drawn ? [box] : [];
    });
    const cells = $derived([
        ...(tab === "geofabrik" ? found.filter((area) => !ticked.includes(area.id)) : []).map((area) => ({ bounds: corners(area.bounds), color: "var(--ink-faint)", label: area.name })),
        ...chosen.map((bounds) => ({ bounds: corners(bounds), color: "var(--running)", label: "the next area" })),
    ]);
    // Every Geofabrik area, not drawn, so that a click picks the smallest that holds the point, also
    // with no search. The map draws only the search results and the ticked areas.
    const regions = $derived(all.map(({ id, name, bounds: b }) => ({ id, name, rings: [rectangle(b)] })));
    function rectangle({ west, south, east, north }: Bbox) {
        return [[south, west], [south, east], [north, east], [north, west], [south, west]] as [number, number][];
    }
    const toggle = (id: string) => (ticked = ticked.includes(id) ? ticked.filter((at) => at !== id) : [...ticked, id]);
    const edge = (value: number) => +value.toFixed(4);
</script>

{#if areas.error}<Failure error={areas.error} />{/if}

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
    <p class="soft">The Fetch line of the review names the extract that the box reads.</p>
{/if}

<DataMap
    {cells}
    outline={boxes.map(corners)}
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
