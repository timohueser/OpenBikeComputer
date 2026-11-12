<!-- The map of the Routes page: the lines of the shown routes from zoom 8, bubbles with a count below it, and the
     selected route wide at each zoom. -->
<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { FeatureCollection, LineString, Point } from 'geojson';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { styleConfig, useMapProtocols } from '../../lib/planner/map-setup';
    import { appearance } from '../../lib/theme.svelte';
    import type { Box, Item } from './filter';

    let { items, shown, selected, onselect, onview, fit }: {
        items: Item[];
        /** The items of the list. The map draws only these, and the bubbles count only these. */
        shown: Item[];
        selected?: string;
        onselect: (id: string) => void;
        onview: (view: Box) => void;
        /** A change fits the map to the shown items. */
        fit: number;
    } = $props();

    const LINE_ZOOM = 8;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>(), ready = $state(false);
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    let styled: typeof theme, fitted = 0;

    const pairs = (line: number[]) => Array.from({ length: line.length / 2 }, (_, i) => [line[2 * i], line[2 * i + 1]]);
    // All lines are in one source; a filter of the layers shows the shown ones, so a chip change sets no data.
    const lines = $derived<FeatureCollection<LineString>>({ type: 'FeatureCollection', features: items.flatMap(({ id, summary }) =>
        summary ? [{ type: 'Feature', properties: { id }, geometry: { type: 'LineString', coordinates: pairs(summary.line) } }] : []) });
    // The bubbles cluster the middle points of the shown items only, so a bubble counts no hidden route.
    const middles = $derived<FeatureCollection<Point>>({ type: 'FeatureCollection', features: shown.flatMap(({ id, summary }) => {
        if (!summary) return [];
        const i = Math.floor(summary.line.length / 4) * 2;
        return [{ type: 'Feature', properties: { id }, geometry: { type: 'Point', coordinates: [summary.line[i], summary.line[i + 1]] } }];
    }) });
    const shownFilter = $derived<maplibregl.FilterSpecification>(['in', ['get', 'id'], ['literal', shown.map(item => item.id)]]);
    const selectedFilter = $derived<maplibregl.FilterSpecification>(['==', ['get', 'id'], selected ?? '']);

    function addLayers() {
        if (!map) return;
        const style = getComputedStyle(container), color = (name: string) => style.getPropertyValue(name).trim();
        const round = { 'line-join': 'round', 'line-cap': 'round' } as const;
        map.addSource('routes', { type: 'geojson', data: untrack(() => lines) });
        map.addSource('middles', { type: 'geojson', data: untrack(() => middles), cluster: true, clusterMaxZoom: LINE_ZOOM - 1, clusterRadius: 48 });
        const shownNow = untrack(() => shownFilter), selectedNow = untrack(() => selectedFilter);
        map.addLayer({ id: 'routes', type: 'line', source: 'routes', minzoom: LINE_ZOOM, filter: shownNow, layout: round,
            paint: { 'line-color': color('--route'), 'line-width': 3, 'line-opacity': 0.75 } });
        // A wide line with no colour, so a click near a line selects it.
        map.addLayer({ id: 'routes-hit', type: 'line', source: 'routes', minzoom: LINE_ZOOM, filter: shownNow, layout: round,
            paint: { 'line-color': '#000', 'line-width': 14, 'line-opacity': 0 } });
        map.addLayer({ id: 'selected-casing', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': color('--panel'), 'line-width': 9 } });
        map.addLayer({ id: 'selected', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': color('--route'), 'line-width': 5 } });
        map.addLayer({ id: 'bubbles', type: 'circle', source: 'middles', maxzoom: LINE_ZOOM,
            paint: { 'circle-color': color('--route'), 'circle-radius': 13, 'circle-stroke-color': color('--panel'), 'circle-stroke-width': 3 } });
        map.addLayer({ id: 'bubble-count', type: 'symbol', source: 'middles', maxzoom: LINE_ZOOM,
            layout: { 'text-field': ['coalesce', ['get', 'point_count_abbreviated'], '1'], 'text-font': ['Noto Sans Regular'], 'text-size': 12, 'text-allow-overlap': true },
            paint: { 'text-color': '#fff' } });
        ready = true;
    }

    function fitShown() {
        const bounds = new maplibregl.LngLatBounds();
        for (const { summary } of untrack(() => shown)) if (summary) bounds.extend(summary.bounds);
        if (bounds.isEmpty() || !map) return;
        const { clientWidth: width, clientHeight: height } = container, side = Math.min(48, width / 8);
        // The bottom keeps the room of the selected card.
        map.fitBounds(bounds, { padding: { top: side, left: side, right: side, bottom: Math.min(160, height / 3) }, maxZoom: 13, animate: false });
    }

    const view = (): Box => {
        const b = map!.getBounds();
        return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    };

    async function zoomIn(event: maplibregl.MapLayerMouseEvent) {
        const bubble = event.features?.[0];
        if (!bubble || bubble.geometry.type !== 'Point') return;
        const cluster = bubble.properties.cluster_id as number | undefined;
        const zoom = cluster === undefined ? LINE_ZOOM : await map!.getSource<maplibregl.GeoJSONSource>('middles')!.getClusterExpansionZoom(cluster);
        map!.easeTo({ center: bubble.geometry.coordinates as [number, number], zoom: Math.max(zoom, map!.getZoom() + 1) });
    }

    onMount(() => {
        useMapProtocols();
        const created = new maplibregl.Map({
            container, style: basemapStyle(styled = theme, styleConfig), bounds: styleConfig.bounds,
            attributionControl: false, maxPitch: 0, renderWorldCopies: false,
        });
        // The selected card covers the bottom of the map.
        created.addControl(new maplibregl.AttributionControl({ compact: true }), 'top-left');
        created.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'top-right');
        created.dragRotate.disable();
        created.touchZoomRotate.disableRotation();
        created.on('style.load', addLayers);
        created.on('load', () => onview(view()));
        created.on('moveend', () => onview(view()));
        created.on('click', 'routes-hit', event => { const id = event.features?.[0]?.properties.id; if (id) onselect(id); });
        created.on('click', 'bubbles', event => void zoomIn(event));
        for (const layer of ['routes-hit', 'bubbles']) {
            created.on('mouseenter', layer, () => { created.getCanvas().style.cursor = 'pointer'; });
            created.on('mouseleave', layer, () => { created.getCanvas().style.cursor = ''; });
        }
        created.on('error', event => console.warn('Routes map:', event.error));
        map = created;
        const resize = new ResizeObserver(() => created.resize());
        resize.observe(container);
        return () => { resize.disconnect(); created.remove(); };
    });

    $effect(() => {
        if (map && theme !== styled) map.setStyle(basemapStyle(styled = theme, styleConfig), { diff: false });
    });
    $effect(() => {
        if (ready) map!.getSource<maplibregl.GeoJSONSource>('routes')?.setData(lines);
    });
    $effect(() => {
        if (!ready) return;
        map!.getSource<maplibregl.GeoJSONSource>('middles')?.setData(middles);
        for (const layer of ['routes', 'routes-hit']) if (map!.getLayer(layer)) map!.setFilter(layer, shownFilter);
    });
    $effect(() => {
        if (ready) for (const layer of ['selected-casing', 'selected']) if (map!.getLayer(layer)) map!.setFilter(layer, selectedFilter);
    });
    $effect(() => {
        const target = fit;
        if (!ready || target === fitted) return;
        fitted = target;
        fitShown();
    });
</script>

<div class="routes-map" bind:this={container}></div>

<style>
    .routes-map {
        width: 100%;
        height: 100%;
        background: var(--parchment);
    }
</style>
