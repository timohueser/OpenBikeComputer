<script lang="ts">
    import { onMount } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { coordinateAt, nearestOnLine, nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { styleConfig, useMapProtocols } from '../../lib/planner/map-setup';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route and the mark. A share is 0 to 1 along the line. */
    let { summary, mark, onpoint }: {
        summary: Summary;
        mark?: number;
        /** The share of the point of the line nearest to the pointer, or null when the pointer is not near the line. */
        onpoint?: (share: number | null) => void;
    } = $props();

    /** The pointer marks the line within this many pixels of it. */
    const NEAR_PX = 24;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>();
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    const coordinates = $derived(Array.from({ length: summary.line.length / 2 }, (_, i): Coordinate => [summary.line[2 * i], summary.line[2 * i + 1]]));
    const dot = Object.assign(document.createElement('div'), { className: 'route-map-mark' });
    const marker = new maplibregl.Marker({ element: dot });
    let shown: typeof theme;

    const bounds = () => summary.bounds as [number, number, number, number];
    const fit = () => map?.fitBounds(bounds(), { padding: Math.min(48, map.getContainer().clientWidth / 12), animate: false });

    function addLine() {
        if (!map) return;
        const style = getComputedStyle(container);
        map.addSource('route', { type: 'geojson', data: { type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } } });
        map.addLayer({ id: 'route-casing', type: 'line', source: 'route', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': style.getPropertyValue('--panel').trim(), 'line-width': 7 } });
        map.addLayer({ id: 'route', type: 'line', source: 'route', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': style.getPropertyValue('--route').trim(), 'line-width': 4 } });
    }

    function point(event: maplibregl.MapMouseEvent) {
        const at: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        const nearest = map!.project(nearestOnLine(coordinates, at).at);
        onpoint?.(Math.hypot(nearest.x - event.point.x, nearest.y - event.point.y) <= NEAR_PX ? nearestProgress(coordinates, at) : null);
    }

    onMount(() => {
        useMapProtocols();
        const created = new maplibregl.Map({
            container, style: basemapStyle(shown = theme, styleConfig), bounds: bounds(), fitBoundsOptions: { padding: 32 },
            attributionControl: { compact: true }, maxPitch: 0, renderWorldCopies: false,
        });
        created.dragRotate.disable();
        created.touchZoomRotate.disableRotation();
        created.on('style.load', addLine);
        created.on('mousemove', point);
        created.on('mouseout', () => onpoint?.(null));
        created.on('error', event => console.warn('Route map:', event.error));
        map = created;
        // A swap of the large and the small place changes the size of the box.
        const resize = new ResizeObserver(() => { created.resize(); fit(); });
        resize.observe(container);
        return () => { resize.disconnect(); marker.remove(); created.remove(); };
    });

    $effect(() => {
        if (map && theme !== shown) map.setStyle(basemapStyle(shown = theme, styleConfig));
    });
    $effect(() => {
        const source = map?.getSource<maplibregl.GeoJSONSource>('route');
        source?.setData({ type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } });
    });
    $effect(() => {
        if (!map) return;
        if (mark === undefined || coordinates.length < 2) marker.remove();
        else marker.setLngLat(coordinateAt(coordinates, mark)).addTo(map);
    });
</script>

<div class="route-map" bind:this={container}></div>

<style>
    .route-map {
        width: 100%;
        height: 100%;
        background: var(--parchment);
    }
    :global(.route-map-mark) {
        width: 14px;
        height: 14px;
        border: 3px solid var(--route);
        border-radius: 50%;
        background: var(--panel);
        pointer-events: none;
    }
</style>
