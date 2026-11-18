<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { coordinateAt, nearestOnLine, nearestProgress, routeSlice, type Coordinate } from '../../lib/planner/geo';
    import Icon from '../../lib/ui/Icon.svelte';
    import PhotoMarks from './PhotoMarks.svelte';
    import PlacePopup from './PlacePopup.svelte';
    import PlaceDisc from '../planner/PlaceDisc.svelte';
    import { mapCallout, type MapCallout } from '../../lib/planner/map-callout';
    import type { RoutePlace } from '../../lib/planner/place-index';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { basemapStyle, type Relief } from '../../lib/planner/map-style';
    import { ClosedAttribution, styleConfig, terrain, useMapProtocols } from '../../lib/planner/map-setup';
    import { terrainRetry } from '../../lib/planner/map-terrain';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route or a ride and the mark. A share is 0 to 1 along the line. */
    let { summary, ride = false, mark, pins = [], photos = [], onphoto, onpoint, fitTo, lit,
        routePlaces = [], litPlaces = $bindable([]), openPlaceId = $bindable(null), corner = [0, 0] }: {
        summary: Summary;
        /** The line is a ride, in `--ride`. */
        ride?: boolean;
        mark?: number;
        /** Labels at shares of the line: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
        /** The photos of a ride at shares of the line, each with the address of its small picture. */
        photos?: { id: string; url: string; share: number }[];
        onphoto?: (id: string) => void;
        /** The share of the point of the line nearest to the pointer, or null when the pointer is not near the line. */
        onpoint?: (share: number | null) => void;
        /** The stretch that the map fits; without it, the whole line. */
        fitTo?: [number, number] | null;
        /** The stretch in the colour of the line; the rest of the line is paler. */
        lit?: [number, number] | null;
        /** Places that the map draws as dots. A lit place and the open place are discs; the open place has its popup. */
        routePlaces?: RoutePlace[];
        litPlaces?: string[];
        openPlaceId?: string | null;
        /** The width and the height of a box in the bottom right corner of the map that the popup keeps clear of. */
        corner?: [number, number];
    } = $props();

    /** The pointer marks the line within NEAR_PX of it. A dot takes the pointer within HOVER_PX, so that its disc does not
     * flicker under the pointer, and a click within CLICK_PX, a target for a finger. The popup keeps MARGIN_PX from the
     * edges of the map, and its tip OFFSET_PX from the place. */
    const NEAR_PX = 24, HOVER_PX = 12, CLICK_PX = 20, MARGIN_PX = 8, OFFSET_PX = 16;
    /** A fit of a short part, such as one of no length, zooms in no further than this. */
    const FIT_ZOOM = 15;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>();
    /** `index` is the place in `pins`; `left` puts the label on the side with more room. */
    let shownPins = $state.raw<{ share: number; label: string; number?: number; x: number; y: number; index: number; left: boolean }[]>([]);
    let shownPhotos = $state.raw<{ id: string; url: string; x: number; y: number }[]>([]);
    let shownDiscs = $state.raw<{ id: string; icon: string; x: number; y: number }[]>([]);
    let popupContent: HTMLDivElement, callout: MapCallout;
    /** The pointer on a dot lit the places, so the map may clear them. */
    let litHere = false;
    const openPlace = $derived(routePlaces.find(({ place }) => place.id === openPlaceId));
    /** The pin whose name shows, by its index; one at a time. */
    let opened = $state<number>();
    const pinName = (pin: { label: string; number?: number }) => pin.number ? pin.label : pin.label ? `Night · ${pin.label}` : 'Night';
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    const coordinates = $derived(lineCoordinates(summary));
    const dot = Object.assign(document.createElement('div'), { className: 'route-map-mark' });
    const marker = new maplibregl.Marker({ element: dot });
    let shown: typeof theme, relief: Relief;

    const bounds = (): [number, number, number, number] => {
        if (!fitTo) return summary.bounds as [number, number, number, number];
        const slice = routeSlice(coordinates, fitTo[0], fitTo[1]);
        const lons = slice.map(c => c[0]), lats = slice.map(c => c[1]);
        return [Math.min(...lons), Math.min(...lats), Math.max(...lons), Math.max(...lats)];
    };
    const fit = (animate = false) => map?.fitBounds(bounds(), { padding: Math.min(48, map.getContainer().clientWidth / 12), maxZoom: FIT_ZOOM, animate, duration: 400 });
    const lineOf = (stretch: [number, number] | null | undefined) =>
        ({ type: 'Feature' as const, properties: {}, geometry: { type: 'LineString' as const, coordinates: stretch ? routeSlice(coordinates, stretch[0], stretch[1]) : [] } });

    /** The colour of the line, a solid paler colour of it for the line outside the lit stretch, and the colour of a place.
     * A canvas gives an opaque colour as `#rrggbb`. */
    function colours() {
        const style = getComputedStyle(container), probe = document.createElement('canvas').getContext('2d')!;
        const line = style.getPropertyValue(ride ? '--ride' : '--route').trim(), panel = style.getPropertyValue('--panel').trim();
        const rgb = (colour: string) => { probe.fillStyle = colour; return [1, 3, 5].map(i => parseInt(String(probe.fillStyle).slice(i, i + 2), 16)); };
        const [a, b] = [rgb(line), rgb(panel)];
        return { panel, line, pale: `rgb(${a.map((v, i) => Math.round(v * .35 + b[i] * .65)).join(',')})`, disc: style.getPropertyValue('--place-disc').trim() };
    }

    const placeData = (places: RoutePlace[]) => ({ type: 'FeatureCollection' as const, features: places.map(({ place }) =>
        ({ type: 'Feature' as const, properties: { id: place.id }, geometry: { type: 'Point' as const, coordinates: place.coordinate } })) });

    /** Runs again after each `setStyle`, so the dots keep their places and take the colours of the theme. */
    function addLine() {
        if (!map) return;
        const { panel, line, pale, disc } = colours();
        map.addSource('route', { type: 'geojson', data: { type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } } });
        map.addSource('lit', { type: 'geojson', data: lineOf(lit) });
        map.addLayer({ id: 'route-casing', type: 'line', source: 'route', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': panel, 'line-width': 7 } });
        map.addLayer({ id: 'route', type: 'line', source: 'route', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': lit ? pale : line, 'line-width': 4 } });
        map.addLayer({ id: 'lit-casing', type: 'line', source: 'lit', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': panel, 'line-width': 8 } });
        map.addLayer({ id: 'lit', type: 'line', source: 'lit', layout: { 'line-join': 'round', 'line-cap': 'round' },
            paint: { 'line-color': line, 'line-width': 5 } });
        // Quiet at the zoom of a long day, readable at the zoom of a town.
        const quiet = ['interpolate', ['linear'], ['zoom'], 9, .6, 12, 1] as maplibregl.ExpressionSpecification;
        map.addSource('places', { type: 'geojson', data: placeData(routePlaces) });
        map.addLayer({ id: 'places', type: 'circle', source: 'places', paint: {
            'circle-color': disc, 'circle-radius': ['interpolate', ['linear'], ['zoom'], 9, 2.5, 14, 4], 'circle-opacity': quiet,
            'circle-stroke-color': panel, 'circle-stroke-width': 1, 'circle-stroke-opacity': quiet } });
    }

    /** The free box of the popup: the map less the margin, and over or beside the corner box, whichever is larger. */
    function free() {
        const { clientWidth: width, clientHeight: height } = container, [cornerWidth, cornerHeight] = corner;
        const whole = { left: MARGIN_PX, top: MARGIN_PX, right: width - MARGIN_PX, bottom: height - MARGIN_PX };
        if (!cornerWidth) return whole;
        const over = { ...whole, bottom: height - cornerHeight - MARGIN_PX }, beside = { ...whole, right: width - cornerWidth - MARGIN_PX };
        const area = (box: typeof whole) => (box.right - box.left) * (box.bottom - box.top);
        return area(over) >= area(beside) ? over : beside;
    }

    /** The id of the dot nearest to `point` within `reach` pixels. */
    function placeAt(point: maplibregl.Point, reach: number): string | undefined {
        if (!map?.getLayer('places')) return undefined;
        const found = map.queryRenderedFeatures([[point.x - reach, point.y - reach], [point.x + reach, point.y + reach]], { layers: ['places'] });
        let best: { id: string; px: number } | undefined;
        for (const feature of found) {
            const at = map.project((feature.geometry as GeoJSON.Point).coordinates as Coordinate), px = Math.hypot(at.x - point.x, at.y - point.y);
            if (px <= reach && (!best || px < best.px)) best = { id: feature.properties.id, px };
        }
        return best?.id;
    }

    /** The pointer on a dot lights its place; off the dots, it clears only what it lit itself, not what the rows lit. */
    function hoverPlace(id: string | undefined) {
        map!.getCanvas().style.cursor = id ? 'pointer' : '';
        if (id && (litPlaces.length !== 1 || litPlaces[0] !== id)) [litPlaces, litHere] = [[id], true];
        else if (!id && litHere) [litPlaces, litHere] = [[], false];
    }

    function point(event: maplibregl.MapMouseEvent) {
        const at: Coordinate = [event.lngLat.lng, event.lngLat.lat];
        const nearest = map!.project(nearestOnLine(coordinates, at).at);
        onpoint?.(Math.hypot(nearest.x - event.point.x, nearest.y - event.point.y) <= NEAR_PX ? nearestProgress(coordinates, at) : null);
    }

    onMount(() => {
        useMapProtocols();
        const lease = terrain.acquire(maplibregl);
        relief = lease.relief();
        const created = new maplibregl.Map({
            container, style: basemapStyle(shown = theme, styleConfig, relief), bounds: bounds(), fitBoundsOptions: { padding: 32 },
            attributionControl: false, maxPitch: 0, renderWorldCopies: false,
        });
        created.addControl(new ClosedAttribution());
        created.dragRotate.disable();
        created.touchZoomRotate.disableRotation();
        created.on('style.load', addLine);
        created.on('mousemove', event => {
            point(event);
            // No dot under a pin or the popup takes the pointer.
            hoverPlace(event.originalEvent.target === created.getCanvas() ? placeAt(event.point, HOVER_PX) : undefined);
        });
        created.on('mouseout', () => { onpoint?.(null); hoverPlace(undefined); });
        created.on('click', event => {
            if (event.originalEvent.target === created.getCanvas()) openPlaceId = placeAt(event.point, CLICK_PX) ?? null;
        });
        const terrainError = terrainRetry(created);
        created.on('error', event => console.warn(terrainError(event) ? 'Route map terrain:' : 'Route map:', event.error));
        callout = mapCallout(created, popupContent, { offset: OFFSET_PX, free });
        map = created;
        // A swap of the large and the small place changes the size of the box. The open popup pans into view after the fit.
        const resize = new ResizeObserver(() => { created.resize(); fit(); callout.refit(true); });
        resize.observe(container);
        return () => { resize.disconnect(); marker.remove(); callout.remove(); created.remove(); lease.release(); };
    });

    $effect(() => {
        const stretch = lit, source = map?.getSource<maplibregl.GeoJSONSource>('lit');
        if (!map || !source) return;
        source.setData(lineOf(stretch));
        const { line, pale } = colours();
        map.setPaintProperty('route', 'line-color', stretch ? pale : line);
    });
    $effect(() => {
        void fitTo?.[0]; void fitTo?.[1];
        if (map) untrack(() => fit(true));
    });
    $effect(() => {
        const data = placeData(routePlaces);
        map?.getSource<maplibregl.GeoJSONSource>('places')?.setData(data);
    });
    $effect(() => {
        const at = openPlace?.place.coordinate ?? null;
        if (map) untrack(() => callout.show(at));
    });

    $effect(() => { dot.classList.toggle('ride', ride); });
    $effect(() => {
        if (map && theme !== shown) map.setStyle(basemapStyle(shown = theme, styleConfig, relief));
    });
    $effect(() => {
        const source = map?.getSource<maplibregl.GeoJSONSource>('route');
        source?.setData({ type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } });
    });
    // The pins, the photos and the discs follow the map: their places in the box change with each move.
    $effect(() => {
        if (!map || coordinates.length < 2) return void ([shownPins, shownPhotos, shownDiscs] = [[], [], []]);
        const shown = map, at = pins.map(pin => coordinateAt(coordinates, pin.share)), photoAt = photos.map(photo => coordinateAt(coordinates, photo.share));
        const discs = routePlaces.filter(({ place }) => place.id === openPlaceId || litPlaces.includes(place.id)).map(({ place }) => place);
        const place = () => {
            const { clientWidth: width, clientHeight: height } = shown.getContainer();
            const inside = ({ x, y }: { x: number; y: number }) => x >= 0 && x <= width && y >= 0 && y <= height;
            shownPins = pins.flatMap((pin, i) => {
                const { x, y } = shown.project(at[i]);
                return inside({ x, y }) ? [{ ...pin, x, y, index: i, left: x > width / 2 }] : [];
            });
            shownPhotos = photos.flatMap(({ id, url }, i) => {
                const { x, y } = shown.project(photoAt[i]);
                return inside({ x, y }) ? [{ id, url, x, y }] : [];
            });
            shownDiscs = discs.flatMap(({ id, category, coordinate }) => {
                const { x, y } = shown.project(coordinate);
                return inside({ x, y }) ? [{ id, icon: placeCategories[category].icon, x, y }] : [];
            });
        };
        place();
        const events = [shown.on('move', place), shown.on('resize', place)];
        return () => events.forEach(event => event.unsubscribe());
    });
    $effect(() => {
        if (!map) return;
        if (mark === undefined || coordinates.length < 2) marker.remove();
        else marker.setLngLat(coordinateAt(coordinates, mark)).addTo(map);
    });
</script>

<svelte:document onpointerdown={event => { if (!(event.target as Element).closest('.route-pin')) opened = undefined; }} />

<div class="route-map" bind:this={container}>
    <!-- Under the pins and the photos; the popup draws over all of them. -->
    <div class="pins">
        {#each shownDiscs as disc (disc.id)}
            <span class="route-disc" style:left={`${disc.x}px`} style:top={`${disc.y}px`}><PlaceDisc icon={disc.icon} grow /></span>
        {/each}
        {#each shownPins as pin (pin.index)}
            <span class="route-pin" style:left={`${pin.x}px`} style:top={`${pin.y}px`}>
                <button type="button" class:night={!pin.number} aria-label={pinName(pin)} aria-expanded={opened === pin.index}
                    onclick={() => opened = pin.index} onfocus={() => opened = pin.index} onblur={() => opened = undefined}
                    onkeydown={event => { if (event.key === 'Escape') opened = undefined; }}>
                    {#if pin.number}{pin.number}{:else}<Icon name="moon" size={12} />{/if}
                </button>
                {#if opened === pin.index}<span class="route-pin-label" class:left={pin.left}>{pinName(pin)}</span>{/if}
            </span>
        {/each}
        <PhotoMarks photos={shownPhotos} onopen={onphoto} />
    </div>
    <div class="popup-storage"><div bind:this={popupContent}>
        {#if openPlace}{#key openPlace.place.id}<PlacePopup routePlace={openPlace} onClose={() => openPlaceId = null} />{/key}{/if}
    </div></div>
</div>

<style>
    /* The popup draws over the discs, the pins and the photos, and under what the page puts over the map. */
    .route-map {
        isolation: isolate;
        width: 100%;
        height: 100%;
        background: var(--parchment);
    }
    .pins {
        position: absolute;
        z-index: 2;
        inset: 0;
        pointer-events: none;
    }
    .pins :global(.route-pin) {
        pointer-events: auto;
    }
    .route-disc {
        position: absolute;
        translate: -50% -50%;
    }
    .popup-storage {
        display: none;
    }
    .route-map :global(.map-callout) {
        z-index: 3;
    }
    /* The credit in the letters and the colours of the page, as on the map of the planner. */
    .route-map :global(.maplibregl-ctrl-attrib) {
        background: var(--panel);
        color: var(--ink-soft);
        font: 11px/20px var(--sans);
    }
    .route-map :global(.maplibregl-ctrl-attrib a) {
        color: inherit;
    }
    :global(:root[data-theme="dark"]) .route-map :global(.maplibregl-ctrl-attrib-button) {
        filter: invert(1);
    }
    :global(.route-map-mark) {
        width: 14px;
        height: 14px;
        border: 3px solid var(--route);
        border-radius: 50%;
        background: var(--panel);
        pointer-events: none;
    }
    :global(.route-map-mark.ride) {
        border-color: var(--ride);
    }
</style>
