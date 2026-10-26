<script lang="ts">
    import { onMount } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { appearance } from '../../lib/theme.svelte';
    import { styleConfig } from './PlannerMap.svelte';

    /** The basemap north up about the eye, with a cone of `fov` degrees along the heading. For orientation only. */
    let { lat, lon, heading, fov }: { lat: number; lon: number; heading: number; fov: number } = $props();

    const R = 44;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>();
    const half = $derived(Math.min(fov, 170) / 2 * Math.PI / 180);
    const cone = $derived(`M0 0L${-R * Math.sin(half)} ${-R * Math.cos(half)}A${R} ${R} 0 0 1 ${R * Math.sin(half)} ${-R * Math.cos(half)}Z`);

    // The planner map registers the `release` and `pmtiles` protocols of the style, and it stays under the window.
    onMount(() => {
        const created = new maplibregl.Map({
            container, style: basemapStyle(appearance.dark ? 'dark' : 'light', styleConfig), center: [lon, lat], zoom: 13,
            interactive: false, attributionControl: false, renderWorldCopies: false, fadeDuration: 0,
        });
        map = created;
        return () => created.remove();
    });
    $effect(() => {
        map?.jumpTo({ center: [lon, lat] });
    });
</script>

<div class="minimap" aria-hidden="true">
    <div class="map" bind:this={container}></div>
    <svg viewBox="-50 -50 100 100">
        <path d={cone} transform={`rotate(${heading})`} />
        <circle r="5" />
    </svg>
</div>

<style>
    .minimap {
        position: absolute;
        left: max(12px, env(safe-area-inset-left));
        bottom: max(12px, env(safe-area-inset-bottom));
        width: 220px;
        height: 140px;
        overflow: hidden;
        border: 3px solid var(--panel);
        border-radius: 8px;
        background: var(--parchment);
        box-shadow: var(--planner-shadow);
        pointer-events: none;
    }
    .map {
        position: absolute;
        inset: 0;
    }
    svg {
        position: absolute;
        top: 50%;
        left: 50%;
        width: 100px;
        height: 100px;
        margin: -50px 0 0 -50px;
    }
    path {
        fill: var(--forest);
        fill-opacity: 0.25;
        stroke: var(--forest);
        stroke-width: 1.5;
        stroke-linejoin: round;
    }
    circle {
        fill: var(--forest);
        stroke: var(--panel);
        stroke-width: 2;
    }
    @media (max-width: 760px) {
        .minimap {
            width: 150px;
            height: 100px;
        }
    }
</style>
