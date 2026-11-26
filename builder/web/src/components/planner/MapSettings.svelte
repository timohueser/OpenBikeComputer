<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import MapControlPanel from './MapControlPanel.svelte';
    import type { OverlayOptions } from '../../lib/planner/route-overlays';
    import type { DataLayer } from '../../lib/planner/layers/data-layer';

    let { contours = $bindable(), slopeAngle = $bindable(), mapOverlays = $bindable(), dataLayers = [], shownLayer = $bindable(''), theme = 'light' }: {
        /** The data layers of the region; the map shows one or none. */
        dataLayers?: DataLayer[];
        /** The id of the shown data layer, or '' for none. */
        shownLayer?: string;
        contours: boolean;
        slopeAngle: boolean;
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
            { id: 'contours', label: 'Contours', on: contours, toggle: () => contours = !contours },
            { id: 'slope-angle', label: 'Slope angle', on: slopeAngle, toggle: () => slopeAngle = !slopeAngle },
            { id: 'closures', label: 'Closures', on: mapOverlays.access, toggle: () => mapOverlays = { ...mapOverlays, access: !mapOverlays.access } },
        ] },
    ].filter(row => row.tiles.length));
</script>

<MapControlPanel label="Map settings">
    {#snippet button()}<Icon name="layers" />{/snippet}
    <div class="settings">
        {#each rows as row (row.label)}
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
        {/each}
    </div>
</MapControlPanel>

<style>
    .settings { display: grid; gap: 12px; }
    button { border: 0; background: none; color: var(--ink); cursor: pointer; }
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
</style>
