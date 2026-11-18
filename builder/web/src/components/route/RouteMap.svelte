<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { coordinateAt, nearestOnLine, nearestProgress, routeSlice, type Coordinate } from '../../lib/planner/geo';
    import Icon from '../../lib/ui/Icon.svelte';
    import OpeningHours from '../planner/OpeningHours.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import PhotoMarks from './PhotoMarks.svelte';
    import { basemapStyle, type Relief } from '../../lib/planner/map-style';
    import { ClosedAttribution, styleConfig, terrain, useMapProtocols } from '../../lib/planner/map-setup';
    import { terrainRetry } from '../../lib/planner/map-terrain';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route or a ride and the mark. A share is 0 to 1 along the line. */
    let { summary, ride = false, mark, pins = [], photos = [], onphoto, onpoint, fitTo, lit, place, dots = [], discs = [], icon = '', popup = null }: {
        dots?: Coordinate[]; discs?: { at: Coordinate; count?: number }[]; icon?: string;
        popup?: { place: import('../../lib/planner/editor').Place; km: string; off: string } | null;
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
        place?: Coordinate | null;
    } = $props();

    /** The pointer marks the line within this many pixels of it. */
    const NEAR_PX = 24;
    /** A fit of a short part, such as one of no length, zooms in no further than this. */
    const FIT_ZOOM = 15;
    let container: HTMLElement;
    let map = $state.raw<maplibregl.Map>();
    /** `index` is the place in `pins`; `left` puts the label on the side with more room. */
    let shownPins = $state.raw<{ share: number; label: string; number?: number; x: number; y: number; index: number; left: boolean }[]>([]);
    let shownPhotos = $state.raw<{ id: string; url: string; x: number; y: number }[]>([]);
    /** The pin whose name shows, by its index; one at a time. */
    let opened = $state<number>();
    const pinName = (pin: { label: string; number?: number }) => pin.number ? pin.label : pin.label ? `Night · ${pin.label}` : 'Night';
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    const coordinates = $derived(lineCoordinates(summary));
    const dot = Object.assign(document.createElement('div'), { className: 'route-map-mark' });
    const marker = new maplibregl.Marker({ element: dot });
    const spot = Object.assign(document.createElement('div'), { className: 'route-map-place' });
    const placeMarker = new maplibregl.Marker({ element: spot });
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

    /** The colour of the line, and a solid paler colour of it for the line outside the lit stretch. A canvas gives an opaque
     * colour as `#rrggbb`. */
    function colours() {
        const style = getComputedStyle(container), probe = document.createElement('canvas').getContext('2d')!;
        const line = style.getPropertyValue(ride ? '--ride' : '--route').trim(), panel = style.getPropertyValue('--panel').trim();
        const rgb = (colour: string) => { probe.fillStyle = colour; return [1, 3, 5].map(i => parseInt(String(probe.fillStyle).slice(i, i + 2), 16)); };
        const [a, b] = [rgb(line), rgb(panel)];
        return { panel, line, pale: `rgb(${a.map((v, i) => Math.round(v * .35 + b[i] * .65)).join(',')})` };
    }

    function addLine() {
        if (!map) return;
        const { panel, line, pale } = colours();
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
        // PROTOTYPE: every place of the chosen kind as a small dot.
        const style = getComputedStyle(container);
        map.addSource('dots', { type: 'geojson', data: dotData(dots) });
        map.addLayer({ id: 'dots', type: 'circle', source: 'dots', paint: {
            'circle-color': style.getPropertyValue('--rust').trim() || '#a4501e', 'circle-stroke-color': panel, 'circle-stroke-width': 1,
            'circle-radius': ['interpolate', ['linear'], ['zoom'], 9, 2.5, 14, 4],
            'circle-opacity': ['interpolate', ['linear'], ['zoom'], 9, 0.6, 12, 1], 'circle-stroke-opacity': ['interpolate', ['linear'], ['zoom'], 9, 0.6, 12, 1] } });
    }
    const dotData = (list: Coordinate[]) => ({ type: 'FeatureCollection' as const, features: list.map(at => ({ type: 'Feature' as const, properties: {}, geometry: { type: 'Point' as const, coordinates: at } })) });
    let shownDiscs = $state.raw<{ x: number; y: number; count?: number; key: string }[]>([]);
    let shownPopup = $state.raw<{ x: number; y: number } | null>(null);
    // PROTOTYPE: pan so that the popup fits above its place, after the fit of the swap.
    $effect(() => {
        const at = popup?.place.coordinate;
        if (!map || !at) return;
        const shown = map, timer = setTimeout(() => {
            const height = shown.getContainer().clientHeight;
            shown.easeTo({ center: at, offset: [0, height / 2 - 70], duration: 350 });
        }, 450);
        return () => clearTimeout(timer);
    });
    $effect(() => { const data = dotData(dots); map?.getSource<maplibregl.GeoJSONSource>('dots')?.setData(data); });
    $effect(() => {
        if (!map) return;
        const shown = map, list = discs, at = popup?.place.coordinate;
        const place = () => {
            shownDiscs = list.map(disc => ({ ...shown.project(disc.at), count: disc.count, key: disc.at.join(',') }));
            shownPopup = at ? shown.project(at) : null;
        };
        place();
        const events = [shown.on('move', place), shown.on('resize', place)];
        return () => events.forEach(event => event.unsubscribe());
    });

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
        created.on('mousemove', point);
        created.on('mouseout', () => onpoint?.(null));
        const terrainError = terrainRetry(created);
        created.on('error', event => console.warn(terrainError(event) ? 'Route map terrain:' : 'Route map:', event.error));
        map = created;
        (window as any).protoMap = created;
        // A swap of the large and the small place changes the size of the box.
        const resize = new ResizeObserver(() => { created.resize(); fit(); });
        resize.observe(container);
        return () => { resize.disconnect(); marker.remove(); placeMarker.remove(); created.remove(); lease.release(); };
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
        if (!map) return;
        if (place) placeMarker.setLngLat(place).addTo(map);
        else placeMarker.remove();
    });

    $effect(() => { dot.classList.toggle('ride', ride); });
    $effect(() => {
        if (map && theme !== shown) map.setStyle(basemapStyle(shown = theme, styleConfig, relief));
    });
    $effect(() => {
        const source = map?.getSource<maplibregl.GeoJSONSource>('route');
        source?.setData({ type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } });
    });
    // The pins and the photos follow the map: their places in the box change with each move.
    $effect(() => {
        if (!map || coordinates.length < 2) return void ([shownPins, shownPhotos] = [[], []]);
        const shown = map, at = pins.map(pin => coordinateAt(coordinates, pin.share)), photoAt = photos.map(photo => coordinateAt(coordinates, photo.share));
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
    <div class="pins">
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
        {#each shownDiscs as disc (disc.key)}
            <span class="proto-disc" style:left={`${disc.x}px`} style:top={`${disc.y}px`}>
                <span class="face"><svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true"><path d={icon} /></svg>
                    {#if disc.count}<b>{disc.count > 99 ? '99+' : disc.count}</b>{/if}</span>
            </span>
        {/each}
        {#if popup && shownPopup}
            <div class="proto-popup" style:left={`${shownPopup.x}px`} style:top={`${shownPopup.y}px`}>
                <strong>{popup.place.label}</strong>
                <small>{popup.place.placeKind ? kindLabel(popup.place.placeKind) : ''}</small>
                <small>{popup.km} · {popup.off}</small>
                <OpeningHours value={popup.place.openingHours ?? 'Mo-Sa 08:00-22:00; Su 10:00-18:00'} />
                <a href="#/">Website</a>
            </div>
        {/if}
    </div>
</div>

<style>
    .route-map {
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
    :global(.route-map-place) {
        width: 12px;
        height: 12px;
        border: 3px solid var(--panel);
        border-radius: 50%;
        background: var(--ink);
        pointer-events: none;
    }
    /* PROTOTYPE: the disc of PlaceMarks.svelte, copied. */
    .proto-disc { position: absolute; z-index: 1; translate: -50% -50%; pointer-events: none; --ease: cubic-bezier(0.23, 1, 0.32, 1); }
    .proto-disc .face {
        position: relative; display: grid; place-items: center; width: 22px; height: 22px; border-radius: 50%;
        color: #fff; background: var(--rust, #a4501e); box-shadow: 0 0 0 1.5px var(--panel, #f4f2eb); animation: grow 120ms var(--ease);
    }
    .proto-disc svg { display: block; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
    .proto-disc b {
        position: absolute; left: 14px; top: -6px; box-sizing: border-box; min-width: 14px; height: 14px; padding: 0 3px; border-radius: 7px;
        color: var(--ink, #1c1b14); background: var(--panel, #fff); font: 700 9px/14px var(--sans, sans-serif); font-variant-numeric: tabular-nums; text-align: center;
        box-shadow: 0 0 0 1px rgb(28 27 20 / 0.15);
    }
    @keyframes grow { from { transform: scale(0.35); } }
    .proto-popup {
        position: absolute; z-index: 3; translate: -50% calc(-100% - 18px); display: flex; flex-direction: column; gap: 4px;
        width: 300px; box-sizing: border-box; padding: 12px 14px; border: 1px solid var(--line-strong); border-radius: 10px;
        background: var(--panel); box-shadow: 0 8px 24px rgb(28 27 20 / 0.22); font-size: 13px; pointer-events: auto;
        animation: pop 160ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .proto-popup small { color: var(--ink-soft); }
    .proto-popup a { color: var(--forest); }
    .proto-popup :global(.hours) { margin: 6px 0 0; padding: 8px 0 0; border-bottom: 0; }
    @keyframes pop { from { opacity: 0; transform: translateY(6px) scale(0.97); } }
    :global(.route-map-mark.ride) {
        border-color: var(--ride);
    }
</style>
