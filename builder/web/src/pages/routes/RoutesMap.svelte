<!-- The map of the Routes page: the lines of the shown routes or rides on the relief from zoom 8, bubbles with a count
     below it, and the selected ones wide at each zoom. -->
<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { FeatureCollection, LineString, Point } from 'geojson';
    import { basemapStyle, type Relief } from '../../lib/planner/map-style';
    import { ClosedAttribution, styleConfig, terrain, useMapProtocols } from '../../lib/planner/map-setup';
    import { terrainRetry } from '../../lib/planner/map-terrain';
    import { appearance } from '../../lib/theme.svelte';
    import type { Box, Item } from './filter';

    let { items, shown, selected, look = 'route', covered = 0, credits = [], together = id => [id], onselect, onview, fit, place }: {
        items: Item[];
        /** The items of the list. The map draws only these, and the bubbles count only these. */
        shown: Item[];
        selected: string[];
        look?: 'route' | 'ride';
        /** The height in pixels of the bottom of the map that the selected card covers. */
        covered?: number;
        /** The credits of the picture of the selected card, which shows none of its own. */
        credits?: string[];
        /** The lines that a click on one line selects: the rides of its row, or the line alone. */
        together?: (id: string) => string[];
        /** Undefined after a click on the map where no route is. */
        onselect: (id: string | undefined) => void;
        onview: (view: Box) => void;
        /** A change fits the map to the shown items. */
        fit: number;
        /** A new place moves the map: a box of some size fits the map, a point shows at zoom 12. */
        place?: { lon: number; lat: number; box?: Box };
    } = $props();

    const LINE_ZOOM = 8;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>(), ready = $state(false), loaded = $state(false);
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    let styled: typeof theme, styledLook: typeof look, fitted = 0, relief: Relief;
    /** The lines that have the feature state `hover`: those that a click at the pointer selects. */
    let hovered: string[] = [];
    /** The credits of the terrain and of the picture, behind a button beside the credit of the map. */
    let moreCredits: HTMLElement, creditsOpen = $state(false);
    const more = $derived([...new Set([styleConfig.terrain_attribution, ...credits])]);

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
    const selectedFilter = $derived<maplibregl.FilterSpecification>(['in', ['get', 'id'], ['literal', selected]]);

    function addLayers() {
        if (!map) return;
        const style = getComputedStyle(container), color = (name: string) => style.getPropertyValue(name).trim();
        const round = { 'line-join': 'round', 'line-cap': 'round' } as const, ride = untrack(() => look) === 'ride';
        const line = color(ride ? '--ride' : '--route');
        map.addSource('routes', { type: 'geojson', data: untrack(() => lines), promoteId: 'id' });
        map.addSource('middles', { type: 'geojson', data: untrack(() => middles), cluster: true, clusterMaxZoom: LINE_ZOOM - 1, clusterRadius: 48 });
        const shownNow = untrack(() => shownFilter), selectedNow = untrack(() => selectedFilter);
        // A feature state changes the paint at once, with no transition and no new data.
        const bolder = (rest: number, under: number): maplibregl.ExpressionSpecification => ['case', ['boolean', ['feature-state', 'hover'], false], under, rest];
        if (ride) {
            map.addLayer({ id: 'casing', type: 'line', source: 'routes', minzoom: LINE_ZOOM, filter: shownNow, layout: round,
                paint: { 'line-color': color('--panel'), 'line-width': 6 } });
        }
        map.addLayer({ id: 'routes', type: 'line', source: 'routes', minzoom: LINE_ZOOM, filter: shownNow, layout: round,
            paint: { 'line-color': line, 'line-width': bolder(3, 4), 'line-opacity': ride ? 1 : bolder(0.75, 1) } });
        // A wide line with no colour, so a click near a line selects it.
        map.addLayer({ id: 'routes-hit', type: 'line', source: 'routes', minzoom: LINE_ZOOM, filter: shownNow, layout: round,
            paint: { 'line-color': '#000', 'line-width': 14, 'line-opacity': 0 } });
        map.addLayer({ id: 'selected-casing', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': color('--panel'), 'line-width': 9 } });
        map.addLayer({ id: 'selected', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': line, 'line-width': 5 } });
        map.addLayer({ id: 'bubbles', type: 'circle', source: 'middles', maxzoom: LINE_ZOOM,
            paint: { 'circle-color': line, 'circle-radius': 13, 'circle-stroke-color': color('--panel'), 'circle-stroke-width': 3 } });
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

    /** Moves the map only when a selected line is under the card or outside the view: it pans, or zooms out. */
    function reveal(ids: string[], bottom: number) {
        const bounds = new maplibregl.LngLatBounds();
        for (const { id, summary } of untrack(() => items)) if (summary && ids.includes(id)) bounds.extend(summary.bounds);
        const { clientWidth: width, clientHeight: height } = container, side = Math.min(48, width / 8);
        if (bounds.isEmpty() || !map || height - bottom < 2 * side) return;
        const sw = map.project(bounds.getSouthWest()), ne = map.project(bounds.getNorthEast());
        if (sw.x >= 0 && ne.x <= width && ne.y >= 0 && sw.y <= height - bottom) return;
        map.fitBounds(bounds, { padding: { top: side, left: side, right: side, bottom: bottom + side / 2 }, maxZoom: map.getZoom() });
    }

    const view = (): Box => {
        const b = map!.getBounds();
        return [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    };

    // The credits of another picture start closed.
    $effect(() => {
        void credits;
        creditsOpen = false;
    });

    async function zoomIn(event: maplibregl.MapLayerMouseEvent) {
        const bubble = event.features?.[0];
        if (!bubble || bubble.geometry.type !== 'Point') return;
        const cluster = bubble.properties.cluster_id as number | undefined;
        const zoom = cluster === undefined ? LINE_ZOOM : await map!.getSource<maplibregl.GeoJSONSource>('middles')!.getClusterExpansionZoom(cluster);
        map!.easeTo({ center: bubble.geometry.coordinates as [number, number], zoom: Math.max(zoom, map!.getZoom() + 1) });
    }

    onMount(() => {
        useMapProtocols();
        styledLook = look;
        const lease = terrain.acquire(maplibregl);
        relief = lease.relief();
        const created = new maplibregl.Map({
            container, style: basemapStyle(styled = theme, styleConfig, relief), bounds: styleConfig.bounds,
            attributionControl: false, maxPitch: 0, renderWorldCopies: false,
        });
        // The selected card covers the bottom of the map.
        created.addControl(new ClosedAttribution({ compact: true }), 'top-left');
        created.addControl({ onAdd: () => moreCredits, onRemove: () => moreCredits.remove() }, 'top-left');
        // The button of the map's credit opens the other credits with it.
        container.querySelector('summary.maplibregl-ctrl-attrib-button')?.addEventListener('click', event =>
            creditsOpen = (event.currentTarget as Element).parentElement!.classList.contains('maplibregl-compact-show'));
        created.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'top-right');
        created.dragRotate.disable();
        created.touchZoomRotate.disableRotation();
        created.on('style.load', addLayers);
        created.on('load', () => { loaded = true; onview(view()); });
        created.on('moveend', () => onview(view()));
        created.on('click', 'routes-hit', event => { const id = event.features?.[0]?.properties.id; if (id) onselect(id); });
        created.on('click', 'bubbles', event => void zoomIn(event));
        created.on('click', event => {
            const layers = ['routes-hit', 'selected-casing', 'bubbles'].filter(layer => created.getLayer(layer));
            if (!layers.length || !created.queryRenderedFeatures(event.point, { layers }).length) onselect(undefined);
        });
        for (const layer of ['routes-hit', 'bubbles']) {
            created.on('mouseenter', layer, () => { created.getCanvas().style.cursor = 'pointer'; });
            created.on('mouseleave', layer, () => { created.getCanvas().style.cursor = ''; });
        }
        // The lines that a click selects are bolder under the pointer. A tap moves a mouse too, and its line would stay bold.
        if (matchMedia('(hover: hover)').matches) {
            const hover = (id?: string) => {
                const next = id === undefined ? [] : together(id);
                if (next.length === hovered.length && next.every((line, i) => line === hovered[i])) return;
                for (const line of hovered) created.setFeatureState({ source: 'routes', id: line }, { hover: false });
                hovered = next;
                for (const line of hovered) created.setFeatureState({ source: 'routes', id: line }, { hover: true });
            };
            created.on('mousemove', 'routes-hit', event => hover(event.features?.[0]?.properties.id));
            created.on('mouseleave', 'routes-hit', () => hover());
            // A move of the map takes the line from under the pointer with no mouse event.
            created.on('movestart', () => hover());
        }
        const terrainError = terrainRetry(created);
        created.on('error', event => console.warn(terrainError(event) ? 'Routes map terrain:' : 'Routes map:', event.error));
        map = created;
        const resize = new ResizeObserver(() => created.resize());
        resize.observe(container);
        return () => { resize.disconnect(); created.remove(); lease.release(); };
    });

    $effect(() => {
        if (map && (theme !== styled || look !== styledLook)) {
            styledLook = look;
            ready = false;
            // The new source has no feature state.
            hovered = [];
            map.setStyle(basemapStyle(styled = theme, styleConfig, relief), { diff: false });
        }
    });
    $effect(() => {
        if (ready) map!.getSource<maplibregl.GeoJSONSource>('routes')?.setData(lines);
    });
    $effect(() => {
        if (!ready) return;
        map!.getSource<maplibregl.GeoJSONSource>('middles')?.setData(middles);
        for (const layer of ['casing', 'routes', 'routes-hit']) if (map!.getLayer(layer)) map!.setFilter(layer, shownFilter);
    });
    $effect(() => {
        if (ready) for (const layer of ['selected-casing', 'selected']) if (map!.getLayer(layer)) map!.setFilter(layer, selectedFilter);
    });
    // A new selection or a new card height moves the map; a reload of the same selection does not.
    let revealed = '';
    $effect(() => {
        const ids = selected, bottom = covered, key = `${ids.join()}|${bottom}`;
        if (!ready || !ids.length || !bottom || key === revealed) return;
        revealed = key;
        untrack(() => reveal(ids, bottom));
    });
    $effect(() => {
        const target = fit;
        if (!ready || target === fitted) return;
        fitted = target;
        fitShown();
    });
    // With no padding, a box that `onview` gave shows the same view again.
    $effect(() => {
        if (!map || !place) return;
        const { lon, lat, box: [west, south, east, north] = [lon, lat, lon, lat] } = place;
        if (west < east && south < north) map.fitBounds([[west, south], [east, north]], { padding: 0, animate: false });
        else map.jumpTo({ center: [lon, lat], zoom: 12 });
    });
</script>

<div class="routes-map" data-loaded={loaded} bind:this={container}></div>
<!-- The map takes this box as a control, after its own credit. The credits of the terrain and of the picture of the
     selected card open with the credit of the map, at its button. -->
<div class="maplibregl-ctrl more-credits" class:open={creditsOpen} bind:this={moreCredits}>
    <p>{@html more.join(' | ')}</p>
</div>

<style>
    .routes-map {
        width: 100%;
        height: 100%;
        background: var(--parchment);
    }
    /* The credits wrap before they reach the zoom buttons at the top right. */
    .routes-map :global(.maplibregl-ctrl-top-left) {
        right: 52px;
    }
    /* The credit in the letters and the colours of the page, as on the map of a route. */
    .routes-map :global(.maplibregl-ctrl-attrib), .more-credits {
        border-radius: 12px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 11px/20px var(--sans);
    }
    .routes-map :global(.maplibregl-ctrl-attrib a), .more-credits :global(a) {
        color: inherit;
        text-decoration: none;
    }
    /* The other credits open and close with the credit of the map, under it. */
    .more-credits {
        clear: both;
        margin-left: 10px;
    }
    .more-credits p {
        display: none;
        margin: 0;
        padding: 4px 10px;
    }
    :global(.maplibregl-compact-show) + .more-credits.open p {
        display: block;
    }
    :global(:root[data-theme="dark"]) .routes-map :global(.maplibregl-ctrl-attrib-button) {
        filter: invert(1);
    }
</style>
