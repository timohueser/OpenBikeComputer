<script lang="ts">
    import { boundsOf, extent, nameOf, parseBox, search, union, type Areas, type Bbox } from "../../lib/data/areas";
    import type { BakeRoute } from "../../lib/data/screens";
    import Icon from "../planner/PlannerIcon.svelte";
    import DataMap from "./DataMap.svelte";

    // The area of a bake: a saved area, a Geofabrik area, a box typed as `W,S,E,N`, or a box drawn
    // on the map. A pick shows on the map and below the list, where its button chooses it. The map
    // moves to a pick from the list, not to a box that the person draws or resizes.
    type Area = NonNullable<BakeRoute["area"]>;
    let {
        areas,
        chosen,
        served,
        serving,
        keep = false,
        choose,
    }: {
        areas: Areas | null;
        chosen: BakeRoute["area"];
        /** The boxes that the destination serves now, and their name, such as `Freiburg on this computer`. */
        served: Bbox[];
        serving: string | null;
        /** The chosen area has a button that keeps it, as when the Bake screen opens the area again. */
        keep?: boolean;
        choose: (area: Area) => void;
    } = $props();

    // svelte-ignore state_referenced_locally
    let picked = $state<Area | null>(chosen);
    // svelte-ignore state_referenced_locally
    let framed = $state<Area | null>(chosen);
    let query = $state("");
    let active = $state(-1);
    let hovered = $state<string | null>(null);
    let drawing = $state(false);

    const boxOf = (area: Area | null) => (!area ? null : area.flag === "--box" ? parseBox(area.value) : areas ? boundsOf(area.value, areas) : null);
    const asBox = ({ west, south, east, north }: Bbox): Area => ({ flag: "--box", value: [west, south, east, north].join(",") });
    const typed = $derived.by(() => {
        const box = parseBox(query.replace(/\s/g, ""));
        return box && box.west < box.east && box.south < box.north && box.west >= -180 && box.east <= 180 && box.south >= -90 && box.north <= 90 ? box : null;
    });
    type Row = { area: Area; name: string; detail: string };
    const rows = $derived.by((): Row[] => {
        if (!areas) return [];
        const hits = query.trim()
            ? search(query, areas, 30)
            : areas.regions.map((region) => ({ id: region.id, name: region.name, detail: "" })).sort((a, b) => a.name.localeCompare(b.name));
        const size = (id: string) => {
            const box = boundsOf(id, areas);
            return box ? extent(box) : "";
        };
        return [
            ...(typed ? [{ area: asBox(typed), name: "Typed box", detail: extent(typed) }] : []),
            ...hits.map((hit) => ({ area: { flag: "--region" as const, value: hit.id }, name: hit.name, detail: [hit.detail, size(hit.id)].filter(Boolean).join(" · ") })),
        ];
    });
    const same = (a: Area | null, b: Area | null) => !!a && !!b && a.flag === b.flag && a.value === b.value;
    const box = $derived(boxOf(picked));
    const name = $derived(!picked ? "" : picked.flag === "--box" ? "A box on the map" : nameOf(picked.value, areas));
    const pointed = $derived(rows.find((row) => row.area.value === hovered) ?? rows[active] ?? null);
    const preview = $derived(pointed && !same(pointed.area, picked) ? boxOf(pointed.area) : null);
    const frame = $derived(boxOf(framed) ?? union(served));

    // The list opens on the chosen area, also when it is far down.
    let list = $state<HTMLElement>();
    let shown = false;
    $effect(() => {
        if (shown || !list || !rows.length) return;
        shown = true;
        // Not `scrollIntoView`, which also scrolls the page.
        const row = list.querySelector<HTMLElement>('[aria-selected="true"]');
        if (row) list.scrollTop = row.offsetTop - list.clientHeight / 2;
    });

    function pick(area: Area) {
        picked = area;
        framed = area;
        drawing = false;
    }
    function keys(event: KeyboardEvent) {
        // Enter picks the row under the arrows, else the first match, and a second Enter chooses it.
        const row = rows[active] ?? (query.trim() ? rows[0] : undefined);
        if (event.key === "ArrowDown") active = Math.min(active + 1, rows.length - 1);
        else if (event.key === "ArrowUp") active = Math.max(active - 1, 0);
        else if (event.key === "Enter" && row && !same(row.area, picked)) pick(row.area);
        else if (event.key === "Enter" && picked && (row || !query.trim())) choose(picked);
        else return;
        event.preventDefault();
        document.getElementById(`area-${active}`)?.scrollIntoView({ block: "nearest" });
    }
</script>

<div class="area">
    <div class="pick">
        <label class="search">
            <span class="label">Country, region or saved area</span>
            <span class="field">
                <Icon name="search" size={16} />
                <input
                    type="search"
                    role="combobox"
                    aria-expanded={rows.length > 0}
                    aria-controls="area-results"
                    aria-autocomplete="list"
                    aria-activedescendant={rows[active] ? `area-${active}` : undefined}
                    placeholder={areas ? "Switzerland, Bayern, or W,S,E,N" : "Reading the areas…"}
                    disabled={!areas}
                    bind:value={query}
                    oninput={() => (active = -1)}
                    onkeydown={keys}
                />
            </span>
        </label>
        <p class="list-head soft">{query.trim() ? `${rows.length ? rows.length : "No"} ${rows.length === 1 ? "match" : "matches"}` : `Saved areas · ${rows.length}`}</p>
        <ul id="area-results" class="results" role="listbox" bind:this={list} aria-label="Areas" onmouseleave={() => (hovered = null)}>
            {#each rows as row, i (row.area.value)}
                <!-- The input takes the keys: an option is chosen with the arrows and Enter, or a click. -->
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <li
                    id="area-{i}"
                    role="option"
                    aria-selected={same(row.area, picked)}
                    class:active={i === active}
                    onmouseenter={() => (hovered = row.area.value)}
                    onclick={() => pick(row.area)}
                >
                    <span class="name">{row.name}</span>
                    {#if row.detail}<span class="detail">{row.detail}</span>{/if}
                    {#if same(row.area, picked)}<Icon name="check" size={16} />{/if}
                </li>
            {:else}
                {#if areas && query.trim()}<li class="empty soft">No area has “{query.trim()}” in its name. Draw a box on the map instead.</li>{/if}
            {/each}
        </ul>
        {#if areas && !areas.index}
            <p class="missing note">Only saved areas: this machine has no Geofabrik index yet. In a terminal: <code>obc data fetch geofabrik-index</code></p>
        {/if}
    </div>

    <div class="stage">
        <DataMap area={box} {preview} outline={served.map((at) => [[at.south, at.west], [at.north, at.east]])} {frame} bind:drawing onarea={(drawn) => (picked = asBox(drawn))} />
        <div class="tools">
            {#if drawing}
                <span class="mode">Drag across the map to draw the area · Esc cancels</span>
                <button type="button" class="tool" onclick={() => (drawing = false)}>Cancel</button>
            {:else}
                <button type="button" class="tool" onclick={() => (drawing = true)}><Icon path="M4 4h16v16H4z" size={16} />{box ? "Draw a new box" : "Draw a box"}</button>
            {/if}
        </div>
        <ul class="legend" aria-label="Map key">
            <li><span class="swatch picked"></span>The area of this bake</li>
            {#if served.length}<li><span class="swatch served"></span>{serving ? `${serving[0].toUpperCase()}${serving.slice(1)}, served now` : "Covered now"}</li>{/if}
            <li><span class="swatch pointed"></span>Under the pointer in the list</li>
        </ul>
    </div>

    <div class="choice" aria-live="polite">
        {#if picked}
            <span class="swatch picked" aria-hidden="true"></span>
            <div class="what">
                <b>{name}{box ? ` · ${extent(box)}` : ""}</b>
                <span class="soft">
                    {#if picked.flag === "--box" && box}West {box.west}, south {box.south}, east {box.east}, north {box.north}. Drag its corners to resize it, its middle to move it.{:else if box}Drag a corner on the map to make a box from it.{/if}
                    {#if serving && !same(picked, chosen)}Replaces {serving}.{/if}
                </span>
            </div>
            {#if keep || !same(picked, chosen)}
                <button type="button" class="primary" onclick={() => choose(picked!)}>{same(picked, chosen) ? "Keep this area" : "Use this area"}<Icon name="arrow" size={16} /></button>
            {/if}
        {:else}
            <p class="soft">Pick an area in the list, type a box as <code>W,S,E,N</code>, or draw a box on the map.</p>
        {/if}
    </div>
</div>

<style>
    .area {
        display: grid;
        grid-template-columns: minmax(260px, 340px) minmax(0, 1fr);
        grid-template-areas: "pick stage" "choice choice";
        gap: 16px 20px;
    }
    .pick {
        grid-area: pick;
        display: flex;
        flex-direction: column;
        gap: 8px;
        min-width: 0;
        /* As tall as the map, so that the list scrolls beside it. */
        height: 480px;
    }
    .label {
        display: block;
        margin-bottom: 6px;
        font-weight: 650;
    }
    .field {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 0 10px;
        background: var(--panel);
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        color: var(--ink-soft);
    }
    .field:focus-within {
        border-color: var(--ink-faint);
        box-shadow: 0 0 0 3px color-mix(in srgb, var(--amber) 35%, transparent);
    }
    input {
        flex: 1;
        min-width: 0;
        min-height: 40px;
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink);
    }
    /* The field shows the focus of its input. */
    input:focus-visible {
        outline: none;
    }
    .list-head {
        margin-top: 4px;
        font-size: 12px;
    }
    .results {
        position: relative;
        flex: 1;
        min-height: 0;
        margin: 0;
        padding: 4px;
        overflow-y: auto;
        list-style: none;
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 8px;
    }
    li[role="option"] {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        column-gap: 8px;
        padding: 7px 10px;
        border-radius: 6px;
        cursor: pointer;
    }
    li[role="option"]:hover,
    li.active {
        background: var(--parchment-2);
    }
    li[aria-selected="true"] {
        background: color-mix(in srgb, var(--amber) 22%, var(--panel));
        box-shadow: inset 3px 0 0 var(--amber);
    }
    li[aria-selected="true"]:hover,
    li[aria-selected="true"].active {
        background: color-mix(in srgb, var(--amber) 30%, var(--panel));
    }
    .name {
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    li[aria-selected="true"] .name {
        font-weight: 650;
    }
    .detail {
        grid-column: 1;
        color: var(--ink-soft);
        font-size: 12px;
        line-height: 16px;
    }
    li :global(svg) {
        grid-column: 2;
        grid-row: 1 / span 2;
        color: var(--ink);
    }
    .empty {
        padding: 10px;
    }
    .note {
        font-size: 13px;
    }
    .stage {
        grid-area: stage;
        position: relative;
        display: flex;
        flex-direction: column;
        gap: 8px;
        min-width: 0;
    }
    .stage :global(.map) {
        height: 480px;
    }
    .tools {
        position: absolute;
        top: 10px;
        right: 10px;
        z-index: 500;
        display: flex;
        align-items: center;
        gap: 8px;
    }
    .tool {
        box-shadow: var(--shadow);
        border-color: transparent;
    }
    .mode {
        padding: 7px 12px;
        background: var(--ink);
        color: var(--panel);
        border-radius: 6px;
        box-shadow: var(--shadow);
        font-weight: 600;
        font-size: 13px;
    }
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 18px;
        margin: 0;
        padding: 0;
        list-style: none;
        font-size: 12px;
        color: var(--ink-soft);
    }
    .legend li {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .swatch {
        flex: none;
        width: 16px;
        height: 12px;
        border-radius: 2px;
    }
    .swatch.picked {
        background: color-mix(in srgb, var(--amber) 25%, transparent);
        border: 2px solid var(--amber);
    }
    .swatch.served {
        border: 2px dotted var(--water);
    }
    .swatch.pointed {
        border: 1.5px dashed var(--ink);
    }
    .choice {
        grid-area: choice;
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 10px 14px;
        min-height: 60px;
        padding: 12px 16px;
        background: var(--parchment);
        border: 1px solid var(--line);
        border-radius: 8px;
    }
    .choice .swatch {
        width: 20px;
        height: 16px;
    }
    .what {
        flex: 1 1 280px;
        display: flex;
        flex-direction: column;
        min-width: 0;
    }
    .what .soft {
        font-size: 13px;
    }
    @media (max-width: 760px) {
        .area {
            grid-template-columns: minmax(0, 1fr);
            grid-template-areas: "pick" "stage" "choice";
        }
        .pick {
            height: 300px;
        }
        .stage :global(.map) {
            height: 340px;
        }
        .choice .primary {
            width: 100%;
        }
    }
</style>
