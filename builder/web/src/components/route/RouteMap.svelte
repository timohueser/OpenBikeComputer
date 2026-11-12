<script lang="ts">
    import { onMount } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { coordinateAt, nearestOnLine, nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import { crowded } from '../../lib/covers/line';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { styleConfig, useMapProtocols } from '../../lib/planner/map-setup';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route and the mark. A share is 0 to 1 along the line. */
    let { summary, mark, pins = [], onpoint }: {
        summary: Summary;
        mark?: number;
        /** Labels at shares of the line: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
        /** The share of the point of the line nearest to the pointer, or null when the pointer is not near the line. */
        onpoint?: (share: number | null) => void;
    } = $props();

    /** The pointer marks the line within this many pixels of it. */
    const NEAR_PX = 24;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>();
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    const coordinates = $derived(lineCoordinates(summary));
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
        if (!map || coordinates.length < 2) return;
        const shown = map, markers = pins.map(pin => new maplibregl.Marker({ element: Object.assign(document.createElement('div'), { className: 'route-pin' }), anchor: 'bottom', offset: [0, -6] })
            .setLngLat(coordinateAt(coordinates, pin.share)).addTo(shown));
        // A crowded waypoint shows its number only, a crowded night nothing.
        const label = () => {
            const short = crowded(markers.map(marker => shown.project(marker.getLngLat())));
            pins.forEach((pin, i) => {
                const element = markers[i].getElement();
                element.classList.toggle('short', short[i]);
                element.hidden = short[i] && !pin.number;
                if (pin.number) element.replaceChildren(Object.assign(document.createElement('b'), { textContent: String(pin.number) }), short[i] ? '' : pin.label);
                else element.textContent = pin.label ? `Night · ${pin.label}` : 'Night';
            });
        };
        label();
        const events = [shown.on('moveend', label), shown.on('resize', label)];
        return () => { events.forEach(event => event.unsubscribe()); markers.forEach(marker => marker.remove()); };
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
