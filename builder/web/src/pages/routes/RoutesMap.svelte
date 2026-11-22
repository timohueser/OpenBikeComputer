<!-- The map of the Routes page: the lines of the shown routes or rides on the relief from zoom 8, bubbles with a count
     below it, and the selected ones wide at each zoom. Signed routes and trails show their lines at each zoom, with no
     bubble. The lines of the hovered row draw over the others, and the hovered and the selected route show where they
     start and which way they go. -->
<script lang="ts" module>
    import type { FeatureCollection as Features } from 'geojson';
    import type * as gl from 'maplibre-gl';

    /** The points and lines that a page draws over the routes, from its own data, and the tip and the click of its markers. */
    export interface MapOverlay {
        /** The features of the source `overlay`, for the lead line of the lit row. */
        data(hovered: string | undefined): Features;
        /** Adds the layers of the source `overlay` and their images to a new style. `colour` reads a colour of the theme. */
        add(map: gl.Map, colour: (name: string) => string): void;
        /** The layers whose features have an `id` that takes a click and shows a tip. */
        hit: string[];
        tip(id: string): { title: string; text: string; action: string } | undefined;
        onclick(id: string): void;
    }
</script>

<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Feature, FeatureCollection, LineString, MultiLineString, Point } from 'geojson';
    import { basemapStyle, type Relief } from '../../lib/planner/map-style';
    import { ClosedAttribution, styleConfig, terrain, useMapProtocols } from '../../lib/planner/map-setup';
    import { terrainRetry } from '../../lib/planner/map-terrain';
    import { SIGNED_GROUND } from '../../lib/planner/route-colors';
    import type { Coordinate } from '../../lib/planner/geo';
    import { appearance } from '../../lib/theme.svelte';
    import { lineColor, type Box, type Item } from './filter';
    import { chevronImage, startOf } from './start';

    let { items, shown, selected, marked = [], look = 'route', covered = 0, credits = [], together = id => [id], hovered, onhover, onselect, onview, fit, place, borders, overlay, pick }: {
        items: Item[];
        /** The items of the list. The map draws only these, and the bubbles count only these. */
        shown: Item[];
        selected: string[];
        /** Lines drawn as selected that the map never moves to: a trail and its parts. */
        marked?: string[];
        /** `signed`: signed routes of the catalog and trails, each in its own colour. Their lines show at each zoom, with
         * no bubble: Explore lists single routes only in views where their lines can be read, so a bubble would count a
         * list that is already out of date. */
        look?: 'route' | 'ride' | 'signed';
        /** The height in pixels of the bottom of the map that the selected card covers. */
        covered?: number;
        /** The credits after the map's own: of the picture of the selected card, which shows none of its own, and of the
         * sources of the routes. */
        credits?: string[];
        /** The lines of the row of a line: the rides of its row, or the line alone. The first is the lead line, which
         * has the start mark. */
        together?: (id: string) => string[];
        /** The lead line of the row that the map lights. */
        hovered?: string;
        /** The line under the pointer, or undefined when the pointer leaves the lines or the map moves under it. */
        onhover?: (id: string | undefined) => void;
        /** Undefined after a click on the map where no route is. */
        onselect: (id: string | undefined) => void;
        /** The first view, and then each view in which the map is still for `STILL_MS`. */
        onview: (view: Box) => void;
        /** A change fits the map to the shown items. */
        fit: number;
        /** A new place moves the map: a box of some size fits the map with `padding` pixels on each side, a point shows at
         * zoom 12. */
        place?: { lon: number; lat: number; box?: Box; padding?: number };
        /** A white dot with a ring in `colour` at each point, over the lines, such as the stage borders of a trail. The map
         * reads them when it adds its layers. A map with them shows no start mark: the dots mark where the lines start. */
        borders?: { points: Coordinate[]; colour: string };
        overlay?: MapOverlay;
        /** While set, a click on the map gives its point and selects nothing; `text` says so over the map. */
        pick?: { text: string; onpick: (at: Coordinate) => void };
    } = $props();

    const LINE_ZOOM = 8;
    const LINE_LAYERS = ['casing', 'signed-casing', 'routes', 'routes-hit'];
    /** A zoom in steps ends a move after each step. The page gets the view of a still map, so that such a gesture gives
     * one view, at its end. */
    const STILL_MS = 100;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>(), ready = $state(false), loaded = $state(false);
    /** The tip of the marker under the pointer, at its point in the map. */
    let tip = $state.raw<{ x: number; y: number; title: string; text: string; action: string }>();
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    let styled: typeof theme, styledLook: typeof look, fitted = 0, relief: Relief;
    /** The credits of the terrain and `credits`, which open with the credit of the map. */
    let moreCredits: HTMLElement;
    const more = $derived([...new Set([styleConfig.terrain_attribution, ...credits])]);

    const pairs = (line: number[]) => Array.from({ length: line.length / 2 }, (_, i) => [line[2 * i], line[2 * i + 1]]);
    // All lines are in one source; a filter of the layers shows the shown ones, so a chip change sets no data.
    const colorOf = (item: Item) => item.kind === 'signed' || item.kind === 'trail' ? lineColor(item) : '';
    // A trail is one feature with a line for each piece of its far line.
    const lineOf = (item: Item, role?: 'hover'): Feature<LineString | MultiLineString>[] => item.summary
        ? [{ type: 'Feature', properties: { id: item.id, color: colorOf(item), role }, geometry: item.kind === 'trail'
            ? { type: 'MultiLineString', coordinates: item.pieces }
            : { type: 'LineString', coordinates: pairs(item.summary.line) } }]
        : [];
    const lines = $derived<FeatureCollection<LineString | MultiLineString>>({ type: 'FeatureCollection', features: items.flatMap(item => lineOf(item)) });
    const itemOf = $derived(new Map(items.map(item => [item.id, item])));
    /** The lit row draws only the lines that the map shows: a chip that hides a line under a still pointer hides its copy. */
    const shownIds = $derived(new Set(shown.map(item => item.id)));
    /** The few features that the hover look and the start mark draw: copies of the lines of the hovered row, and the start
     * of the lead line of the hovered and of the selected row. A trail has no start: its far pieces have no order. */
    const focusData = $derived.by<FeatureCollection>(() => {
        const lit = hovered === undefined ? [] : together(hovered).filter(id => shownIds.has(id));
        const features: Feature[] = lit.flatMap(id => { const item = itemOf.get(id); return item ? lineOf(item, 'hover') : []; });
        for (const id of borders ? [] : new Set([hovered !== undefined && shownIds.has(hovered) ? hovered : undefined, selected[0]])) {
            const item = id === undefined ? undefined : itemOf.get(id), start = item?.summary && item.kind !== 'trail' ? startOf(item.summary.line) : undefined;
            if (start) features.push({ type: 'Feature', properties: { color: colorOf(item!), role: 'start', bearing: start.bearing }, geometry: { type: 'Point', coordinates: start.at } });
        }
        return { type: 'FeatureCollection', features };
    });
    // The bubbles cluster the middle points of the shown items only, so a bubble counts no hidden route.
    const middles = $derived<FeatureCollection<Point>>({ type: 'FeatureCollection', features: shown.flatMap(({ summary }) => {
        if (!summary) return [];
        const i = Math.floor(summary.line.length / 4) * 2;
        return [{ type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: [summary.line[i], summary.line[i + 1]] } }];
    }) });
    const shownFilter = $derived<maplibregl.FilterSpecification>(['in', ['get', 'id'], ['literal', shown.map(item => item.id)]]);
    const selectedFilter = $derived<maplibregl.FilterSpecification>(['in', ['get', 'id'], ['literal', [...selected, ...marked]]]);
    /** The width of a signed line: thin where a wide view shows many routes, wider where the rider follows one. `edge`
     * is the light edge of the dark theme, on both sides. */
    const signedWidth = (edge = 0): maplibregl.ExpressionSpecification => ['interpolate', ['linear'], ['zoom'],
        9, 2 + edge, 12, 3 + edge, 15, 4 + edge];
    /** While a signed route is selected, the other lines dim. */
    const signedOpacity = (dim: boolean) => dim ? 0.35 : 0.9;

    function addLayers() {
        if (!map) return;
        const style = getComputedStyle(container), color = (name: string) => style.getPropertyValue(name).trim();
        const round = { 'line-join': 'round', 'line-cap': 'round' } as const, ride = untrack(() => look) === 'ride';
        const signed = untrack(() => look) === 'signed', dim = untrack(() => selected.length + marked.length) > 0, minzoom = signed ? 0 : LINE_ZOOM;
        const line: maplibregl.ExpressionSpecification | string = signed ? ['get', 'color'] : color(ride ? '--ride' : '--route');
        // The set of route colours is too dark for the dark map, so a signed line there has a light edge.
        const edge = signed && untrack(() => theme) === 'dark' ? SIGNED_GROUND : color('--panel');
        map.addSource('routes', { type: 'geojson', data: untrack(() => lines) });
        const shownNow = untrack(() => shownFilter), selectedNow = untrack(() => selectedFilter);
        if (ride) {
            map.addLayer({ id: 'casing', type: 'line', source: 'routes', minzoom, filter: shownNow, layout: round,
                paint: { 'line-color': color('--panel'), 'line-width': 6 } });
        }
        if (edge === SIGNED_GROUND) {
            map.addLayer({ id: 'signed-casing', type: 'line', source: 'routes', minzoom, filter: shownNow, layout: round,
                paint: { 'line-color': edge, 'line-width': signedWidth(2), 'line-opacity': signedOpacity(dim) } });
        }
        map.addLayer({ id: 'routes', type: 'line', source: 'routes', minzoom, filter: shownNow, layout: round,
            paint: { 'line-color': line, 'line-width': signed ? signedWidth() : 3, 'line-opacity': ride ? 1 : signed ? signedOpacity(dim) : 0.75 } });
        // A wide line with no colour, so a click near a line selects it.
        map.addLayer({ id: 'routes-hit', type: 'line', source: 'routes', minzoom, filter: shownNow, layout: round,
            paint: { 'line-color': '#000', 'line-width': 14, 'line-opacity': 0 } });
        // The hover look and the start mark draw only the features of `focus`, so their frame cost does not grow with
        // the routes in view.
        map.addSource('focus', { type: 'geojson', data: untrack(() => focusData) });
        const role = (name: string): maplibregl.FilterSpecification => ['==', ['get', 'role'], name];
        map.addLayer({ id: 'hover-casing', type: 'line', source: 'focus', minzoom, filter: role('hover'), layout: round,
            paint: { 'line-color': edge, 'line-width': 8 } });
        map.addLayer({ id: 'hover', type: 'line', source: 'focus', minzoom, filter: role('hover'), layout: round,
            paint: { 'line-color': line, 'line-width': 4 } });
        map.addLayer({ id: 'selected-casing', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': edge, 'line-width': 9 } });
        map.addLayer({ id: 'selected', type: 'line', source: 'routes', filter: selectedNow, layout: round,
            paint: { 'line-color': line, 'line-width': 5 } });
        const dots = untrack(() => borders);
        if (dots) {
            const features = dots.points.map((point): Feature<Point> => ({ type: 'Feature', properties: {}, geometry: { type: 'Point', coordinates: point } }));
            map.addSource('borders', { type: 'geojson', data: { type: 'FeatureCollection', features } });
            map.addLayer({ id: 'borders', type: 'circle', source: 'borders',
                paint: { 'circle-radius': 4, 'circle-color': '#fff', 'circle-stroke-width': 2.5, 'circle-stroke-color': dots.colour } });
        }
        // A new style drops the images of the last one.
        if (!map.hasImage('obc-start-arrow')) map.addImage('obc-start-arrow', chevronImage(), { pixelRatio: 2 });
        map.addLayer({ id: 'start-dot', type: 'circle', source: 'focus', minzoom, filter: role('start'),
            paint: { 'circle-radius': 5, 'circle-color': '#fff', 'circle-stroke-width': 3, 'circle-stroke-color': line } });
        // The offset turns with the icon and is in units of its size: the arrow sits 14 px ahead of the dot at each zoom.
        map.addLayer({ id: 'start-arrow', type: 'symbol', source: 'focus', minzoom, filter: role('start'),
            layout: { 'icon-image': 'obc-start-arrow', 'icon-size': 0.9, 'icon-rotate': ['get', 'bearing'], 'icon-rotation-alignment': 'map',
                'icon-offset': [0, -14 / 0.9], 'icon-allow-overlap': true, 'icon-ignore-placement': true } });
        if (!signed) {
            map.addSource('middles', { type: 'geojson', data: untrack(() => middles), cluster: true, clusterMaxZoom: LINE_ZOOM - 1, clusterRadius: 48 });
            map.addLayer({ id: 'bubbles', type: 'circle', source: 'middles', maxzoom: LINE_ZOOM,
                paint: { 'circle-color': line, 'circle-radius': 13, 'circle-stroke-color': color('--panel'), 'circle-stroke-width': 3 } });
            map.addLayer({ id: 'bubble-count', type: 'symbol', source: 'middles', maxzoom: LINE_ZOOM,
                layout: { 'text-field': ['coalesce', ['get', 'point_count_abbreviated'], '1'], 'text-font': ['Noto Sans Regular'], 'text-size': 12, 'text-allow-overlap': true },
                paint: { 'text-color': '#fff' } });
        }
        // The overlay adds its layers to this source when it comes.
        map.addSource('overlay', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
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
        created.addControl(new ClosedAttribution(), 'top-left');
        created.addControl({ onAdd: () => moreCredits, onRemove: () => moreCredits.remove() }, 'top-left');
        created.addControl(new maplibregl.NavigationControl({ showCompass: false }), 'top-right');
        created.dragRotate.disable();
        created.touchZoomRotate.disableRotation();
        created.on('style.load', addLayers);
        // The page gets its first view with the style: the load of the map waits for each tile of the view.
        created.once('style.load', () => onview(view()));
        created.on('load', () => { loaded = true; });
        let still: ReturnType<typeof setTimeout> | undefined;
        created.on('movestart', () => clearTimeout(still));
        created.on('moveend', () => {
            clearTimeout(still);
            still = setTimeout(() => onview(view()), STILL_MS);
        });
        /** The first feature of `layers` at a point. */
        const featureAt = (at: maplibregl.PointLike, layers: string[]) => {
            const present = layers.filter(layer => created.getLayer(layer));
            return present.length ? created.queryRenderedFeatures(at, { layers: present })[0] : undefined;
        };
        /** The words of the last tip, for the marker under the pointer. A click, a move or the pointer leaving forgets them, and
         * so does a new overlay, which a new query gives. */
        let told: { overlay: MapOverlay; id: string; about: ReturnType<MapOverlay['tip']> } | undefined;
        // A marker of the overlay lies over the lines, so it takes the click first.
        created.on('click', event => {
            tip = told = undefined;
            if (pick) return pick.onpick([event.lngLat.lng, event.lngLat.lat]);
            const marker = overlay && featureAt(event.point, overlay.hit)?.properties.id;
            if (marker !== undefined) return overlay!.onclick(String(marker));
            const line = featureAt(event.point, ['routes-hit']);
            if (line) onselect(line.properties.id);
            else if (!featureAt(event.point, ['selected-casing', 'bubbles'])) onselect(undefined);
        });
        created.on('click', 'bubbles', event => void zoomIn(event));
        for (const layer of ['routes-hit', 'bubbles']) {
            created.on('mouseenter', layer, () => { created.getCanvas().style.cursor = 'pointer'; });
            created.on('mouseleave', layer, () => { created.getCanvas().style.cursor = ''; });
        }
        // A tap moves a mouse too, and its line would stay lit.
        if (matchMedia('(hover: hover)').matches) {
            let sent: string | undefined;
            const hover = (id?: string) => {
                if (id !== sent) onhover?.(sent = id);
            };
            created.on('mousemove', 'routes-hit', event => hover(event.features?.[0]?.properties.id));
            created.on('mouseleave', 'routes-hit', () => hover());
            // A move of the map takes the line from under the pointer with no mouse event.
            created.on('movestart', () => hover());
            let onMarker = false;
            created.on('mousemove', event => {
                const part = pick ? undefined : overlay, found = part && featureAt(event.point, part.hit)?.properties.id;
                const id = found === undefined ? undefined : String(found);
                if (id !== undefined && (told?.overlay !== part || told?.id !== id)) told = { overlay: part!, id, about: part!.tip(id) };
                const about = id === undefined ? undefined : told!.about;
                // The tip stays inside the map.
                tip = about && { x: Math.min(Math.max(event.point.x, 130), container.clientWidth - 130), y: Math.max(event.point.y, 76), ...about };
                // The lines set the cursor too: only a change of the marker under the pointer sets it.
                if ((id !== undefined) !== onMarker) created.getCanvas().style.cursor = (onMarker = id !== undefined) ? 'pointer' : '';
            });
            for (const event of ['mouseout', 'movestart'] as const) created.on(event, () => { tip = told = undefined; });
        }
        const terrainError = terrainRetry(created);
        created.on('error', event => {
            console.warn(terrainError(event) ? 'Routes map terrain:' : 'Routes map:', event.error);
            // A failed tile draws no frame, and only a frame tells the map that it has loaded.
            created.triggerRepaint();
        });
        map = created;
        const resize = new ResizeObserver(() => created.resize());
        resize.observe(container);
        return () => { clearTimeout(still); resize.disconnect(); created.remove(); lease.release(); };
    });

    $effect(() => {
        if (map && (theme !== styled || look !== styledLook)) {
            styledLook = look;
            ready = false;
            map.setStyle(basemapStyle(styled = theme, styleConfig, relief), { diff: false });
        }
    });
    $effect(() => {
        if (ready) map!.getSource<maplibregl.GeoJSONSource>('routes')?.setData(lines);
    });
    $effect(() => {
        if (ready) map!.getSource<maplibregl.GeoJSONSource>('focus')?.setData(focusData);
    });
    $effect(() => {
        if (!ready) return;
        const part = overlay;
        if (part && !map!.getLayer(part.hit[0])) {
            const style = getComputedStyle(container);
            part.add(map!, name => style.getPropertyValue(name).trim());
        }
        map!.getSource<maplibregl.GeoJSONSource>('overlay')?.setData(part?.data(hovered) ?? { type: 'FeatureCollection', features: [] });
    });
    $effect(() => {
        if (!ready) return;
        if (look !== 'signed') map!.getSource<maplibregl.GeoJSONSource>('middles')?.setData(middles);
        for (const layer of LINE_LAYERS) if (map!.getLayer(layer)) map!.setFilter(layer, shownFilter);
    });
    $effect(() => {
        if (!ready) return;
        for (const layer of ['selected-casing', 'selected']) if (map!.getLayer(layer)) map!.setFilter(layer, selectedFilter);
        if (look !== 'signed') return;
        for (const layer of ['signed-casing', 'routes']) if (map!.getLayer(layer)) map!.setPaintProperty(layer, 'line-opacity', signedOpacity(selected.length + marked.length > 0));
    });
    // A new selection or a new card height moves the map; a reload of the same selection does not, so a pan of the rider
    // stays.
    let revealed = '';
    $effect(() => {
        const ids = selected, bottom = covered, key = `${ids.join()}|${bottom}`;
        if (!ready || !ids.length || key === revealed) return;
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
        const { lon, lat, box: [west, south, east, north] = [lon, lat, lon, lat], padding = 0 } = place;
        if (west < east && south < north) map.fitBounds([[west, south], [east, north]], { padding, animate: false });
        else map.jumpTo({ center: [lon, lat], zoom: 12 });
    });
</script>

<div class="routes-map" class:picking={!!pick} data-loaded={loaded} bind:this={container}></div>
{#if pick}<p class="pick">{pick.text}</p>{/if}
{#if tip}
    <p class="tip" style:left="{tip.x}px" style:top="{tip.y}px"><b>{tip.title}</b>{tip.text}<em>{tip.action}</em></p>
{/if}
<!-- The map takes this box as a control, after its own credit. The credits of the terrain and `credits` open with the
     credit of the map, at its button. -->
<div class="maplibregl-ctrl more-credits" bind:this={moreCredits}>
    <p>{@html more.join(' | ')}</p>
</div>

<style>
    .routes-map {
        width: 100%;
        height: 100%;
        background: var(--parchment);
    }
    /* The credits wrap before they reach the zoom buttons at the top right, and the buttons that the page puts beside
       them (`--map-buttons`). */
    .routes-map :global(.maplibregl-ctrl-top-left) {
        right: calc(52px + var(--map-buttons, 0px));
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
    :global(.maplibregl-compact-show) + .more-credits p {
        display: block;
    }
    .picking :global(canvas) {
        cursor: crosshair !important;
    }
    .pick, .tip {
        position: absolute;
        z-index: 2;
        margin: 0;
        font-size: 13px;
        pointer-events: none;
    }
    .pick {
        top: 12px;
        left: 50%;
        padding: 6px 12px;
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink);
        white-space: nowrap;
        box-shadow: var(--planner-shadow);
        transform: translateX(-50%);
    }
    /* Over the point, in the dark look of the tips of the page in both themes. */
    .tip {
        padding: 5px 10px;
        border-radius: 7px;
        background: rgb(28 27 20 / 0.92);
        color: #fff;
        line-height: 1.35;
        white-space: nowrap;
        transform: translate(-50%, calc(-100% - 14px));
    }
    .tip b, .tip em {
        display: block;
    }
    .tip b {
        font-weight: 600;
    }
    .tip em {
        color: #f4d58d;
        font-style: normal;
    }
    :global(:root[data-theme="dark"]) .routes-map :global(.maplibregl-ctrl-attrib-button) {
        filter: invert(1);
    }
</style>
