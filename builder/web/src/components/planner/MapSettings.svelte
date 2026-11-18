<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import { bandShare, networkLevels, type OverlayOptions } from '../../lib/planner/route-overlays';
    import type { DataLayer } from '../../lib/planner/layers/data-layer';

    let { hillshade = $bindable(), contours = $bindable(), mapOverlays = $bindable(), dataLayers = [], shownLayer = $bindable(''), theme = 'light' }: {
        /** The data layers of the region; the map shows one or none. */
        dataLayers?: DataLayer[];
        /** The id of the shown data layer, or '' for none. */
        shownLayer?: string;
        hillshade: boolean;
        contours: boolean;
        mapOverlays: OverlayOptions;
        theme?: 'light' | 'dark';
    } = $props();

    /** Crops of the planner map with one layer on, made by `tests/browser/planner/tile-pictures.mjs`. */
    const pictures = import.meta.glob<string>('./tiles/*.jpg', { eager: true, query: '?url', import: 'default' });

    interface Tile { id: string; label: string; on: boolean; toggle: () => void; icon?: string }

    const networks = [{ id: 'cycling', label: 'Cycling', icon: 'touring' }, { id: 'hiking', label: 'Hiking', icon: 'hiking' }, { id: 'mtb', label: 'MTB', icon: 'mtb' }] as const;
    const rows = $derived<{ label: string; tiles: Tile[] }[]>([
        { label: 'Routes, one at a time', tiles: networks.map(({ id, label, icon }) => ({ id, label, icon, on: mapOverlays.network === id,
            toggle: () => mapOverlays = { ...mapOverlays, network: mapOverlays.network === id ? 'none' : id } })) },
        { label: 'Conditions, one at a time', tiles: dataLayers.map(({ id, label }) => ({ id, label, on: shownLayer === id, toggle: () => shownLayer = shownLayer === id ? '' : id })) },
        { label: 'Map', tiles: [
            { id: 'relief', label: 'Relief', on: hillshade, toggle: () => hillshade = !hillshade },
            { id: 'contours', label: 'Contours', on: contours, toggle: () => contours = !contours },
            { id: 'closures', label: 'Closures', on: mapOverlays.access, toggle: () => mapOverlays = { ...mapOverlays, access: !mapOverlays.access } },
        ] },
    ].filter(row => row.tiles.length));

    let open = $state(false);
    let root: HTMLDivElement;

    function outside(event: PointerEvent) {
        if (open && !root.contains(event.target as Node)) open = false;
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.stopPropagation();
        open = false;
        root.querySelector<HTMLElement>('.toggle')?.focus();
    }
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="map-settings" bind:this={root} onkeydown={key}>
    <button type="button" class="toggle" class:chosen={open} aria-label="Map settings" aria-expanded={open} aria-haspopup="dialog" onclick={() => open = !open}><Icon name="layers" /></button>
    {#if open}
        <div class="panel" role="dialog" aria-label="Map settings" tabindex="-1">
            {#each rows as row, i (row.label)}
                <div class="row" role="group" aria-label={row.label}>
                    {#each row.tiles as tile (tile.id)}
                        {@const src = pictures[`./tiles/${tile.id}-${theme}.jpg`]}
                        <button type="button" class="tile" aria-pressed={tile.on} onclick={tile.toggle}>
                            <span class="picture">
                                {#if src}<img {src} alt="" draggable="false" />{/if}
                                {#if tile.icon}<span class="badge" aria-hidden="true"><Icon name={tile.icon} size={14} /></span>{/if}
                                <span class="check" aria-hidden="true"><Icon name="check" size={11} /></span>
                            </span>
                            <span class="name">{tile.label}</span>
                        </button>
                    {/each}
                </div>
                {#if i === 0 && mapOverlays.network !== 'none'}
                    <ul class="legend" aria-label="Route network colors" style:--band-share={`${bandShare[theme] * 100}%`}>
                        {#each networkLevels as level (level.rank)}<li title={level.label}><span class="sample" style:color={theme === 'dark' ? level.dark : level.color}></span>{level.rank === 3 ? 'National / intl.' : level.rank === 0 ? 'Unspecified' : level.label}</li>{/each}
                    </ul>
                {/if}
            {/each}
        </div>
    {/if}
</div>

<style>
    .map-settings {
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
    }
    button { border: 0; background: none; color: var(--ink); cursor: pointer; }
    .toggle {
        display: grid;
        place-items: center;
        width: 36px;
        height: 36px;
        padding: 0;
        border-radius: 8px;
    }
    .toggle:hover,
    .toggle.chosen {
        color: var(--link);
    }
    /* Positioned against the controls column, so it starts level with the first control and stays inside the map. */
    .panel {
        position: absolute;
        top: 0;
        right: 44px;
        display: grid;
        gap: 12px;
        width: min(288px, calc(100vw - 80px));
        box-sizing: border-box;
        max-height: calc(var(--map-height, 100vh) - 32px);
        overflow: auto;
        padding: 12px;
        border-radius: 8px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        font-size: 13px;
    }
    .row { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
    .tile { display: grid; gap: 4px; padding: 0; border-radius: 7px; text-align: center; }
    .picture { position: relative; display: block; aspect-ratio: 3 / 2; border-radius: 7px; background: var(--parchment-2); }
    .picture img { display: block; width: 100%; height: 100%; border-radius: inherit; object-fit: cover; }
    /* The edge lies over the picture, so a pale crop keeps its shape on the panel. */
    .picture::after { content: ''; position: absolute; inset: 0; border-radius: inherit; box-shadow: inset 0 0 0 1px var(--line-strong); }
    .tile:hover .picture::after { box-shadow: inset 0 0 0 1px var(--ink-soft); }
    .name { font-size: 12px; line-height: 16px; font-weight: 500; color: var(--ink-soft); }
    .badge, .check { position: absolute; z-index: 1; display: grid; place-items: center; border-radius: 50%; }
    .badge { bottom: 3px; left: 3px; width: 20px; height: 20px; background: var(--panel); color: var(--ink); }
    .check { top: 4px; right: 4px; width: 17px; height: 17px; background: var(--link); color: var(--panel); visibility: hidden; }
    .check :global(svg) { stroke-width: 3; }
    /* A lit tile: a ring, a check and a darker, heavier word, so the state does not rely on colour. The focus outline of the app
       goes around the whole tile, and a tile shows one frame: with the focus, the ring goes. */
    .tile[aria-pressed="true"]:not(:focus-visible) .picture { outline: 2px solid var(--link); outline-offset: 1px; }
    .tile[aria-pressed="true"] .check { visibility: visible; }
    .tile[aria-pressed="true"] .name { font-weight: 650; color: var(--ink); }
    /* A band with its line: the map draws a route network as a line when far out and as a band when close. */
    .sample { flex: 0 0 14px; height: 8px; border-radius: 2px; background: linear-gradient(currentColor 0 0) center / 100% 2px no-repeat, color-mix(in srgb, currentColor var(--band-share), var(--parchment)); }
    .legend { display: grid; grid-template-columns: 1fr 1fr; column-gap: 8px; margin-top: -4px; font-size: 12px; color: var(--ink-soft); }
    .legend li { justify-content: flex-start; min-height: 22px; }
    ul {
        margin: 0;
        padding: 0;
        list-style: none;
    }
    li {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 8px;
    }
</style>
