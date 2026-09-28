<script lang="ts">
    import PlannerMap from "./PlannerMap.svelte";
    import { MAP_VIEWS } from "../../lib/planner/map-data";
    let theme = $state<"light" | "dark">("light");
    let relief = $state(true);
    let contours = $state(true);
    let view = $state(0);
    let map: PlannerMap;
    $effect(() => { document.documentElement.dataset.theme = theme; });
</script>

<main>
    <header>
        <div><h1>Outdoor map study</h1><p>Light and Dusk · real vector tiles and terrain</p></div>
        <a href="/planner.html">Open desktop planner</a>
    </header>
    <nav aria-label="Map appearance">
        <label>View <select bind:value={view} onchange={() => map?.showPlace(MAP_VIEWS[view].center, MAP_VIEWS[view].zoom)}>{#each MAP_VIEWS as option, i}<option value={i}>{option.name}</option>{/each}</select></label>
        <label>Theme <select bind:value={theme}><option value="light">Light</option><option value="dark">Dusk</option></select></label>
        <label><input type="checkbox" bind:checked={relief} /> Hillshade</label>
        <label><input type="checkbox" bind:checked={contours} /> Contours</label>
    </nav>
    <section aria-label="Map style preview"><PlannerMap bind:this={map} {theme} hillshade={relief} {contours} center={MAP_VIEWS[0].center} zoom={MAP_VIEWS[0].zoom} /></section>
    <footer><span>Warm through-roads · violet cycleways · teal contours</span><span>Evaluation sources: Protomaps + Mapterhorn. Terrain detail capped at z12. Scroll to zoom.</span></footer>
</main>

<style>
    :global(body) { margin: 0; font-family: var(--sans); background: var(--parchment); color: var(--ink); }
    main { height: 100dvh; display: grid; grid-template-rows: auto auto 1fr auto; }
    header { display: flex; justify-content: space-between; align-items: center; padding: 16px 24px; border-bottom: 1px solid var(--line); }
    h1 { font-size: 20px; margin: 0; font-weight: 650; }
    p { margin: 4px 0 0; font-size: 13px; color: var(--ink-soft); }
    a { color: var(--ink); text-underline-offset: 3px; font-size: 14px; }
    nav { display: flex; flex-wrap: wrap; align-items: center; gap: 24px; padding: 12px 24px; }
    label { display: flex; align-items: center; gap: 8px; font-size: 14px; }
    select { font: inherit; color: var(--ink); background: var(--panel); border: 1px solid var(--line-strong); border-radius: 4px; padding: 6px 8px; }
    input { accent-color: var(--rust); }
    section { min-height: 0; }
    footer { padding: 10px 24px; display: flex; flex-wrap: wrap; gap: 12px; justify-content: space-between; color: var(--ink-soft); font-size: 12px; border-top: 1px solid var(--line); }
</style>
