<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { coordinateAt, nearestOnLine, nearestProgress, routeSlice, type Coordinate } from '../../lib/planner/geo';
    import Icon from '../planner/PlannerIcon.svelte';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { styleConfig, useMapProtocols } from '../../lib/planner/map-setup';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route or a ride and the mark. A share is 0 to 1 along the line. */
    let { summary, ride = false, mark, pins = [], onpoint, fitTo, lit, place }: {
        summary: Summary;
        /** The line is a ride, in `--ride`. */
        ride?: boolean;
        mark?: number;
        /** Labels at shares of the line: a numbered waypoint, or a night without a number. */
        pins?: { share: number; label: string; number?: number }[];
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
    /** The pin whose name shows, by its index; one at a time. */
    let opened = $state<number>();
    const pinName = (pin: { label: string; number?: number }) => pin.number ? pin.label : pin.label ? `Night · ${pin.label}` : 'Night';
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    const coordinates = $derived(lineCoordinates(summary));
    const dot = Object.assign(document.createElement('div'), { className: 'route-map-mark' });
    const marker = new maplibregl.Marker({ element: dot });
    const spot = Object.assign(document.createElement('div'), { className: 'route-map-place' });
    const placeMarker = new maplibregl.Marker({ element: spot });
    let shown: typeof theme;

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
        return () => { resize.disconnect(); marker.remove(); placeMarker.remove(); created.remove(); };
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
        if (map && theme !== shown) map.setStyle(basemapStyle(shown = theme, styleConfig));
    });
    $effect(() => {
        const source = map?.getSource<maplibregl.GeoJSONSource>('route');
        source?.setData({ type: 'Feature', properties: {}, geometry: { type: 'LineString', coordinates } });
    });
    // The pins follow the map: their places in the box change with each move.
    $effect(() => {
        if (!map || coordinates.length < 2) return void (shownPins = []);
        const shown = map, at = pins.map(pin => coordinateAt(coordinates, pin.share));
        const place = () => {
            const { clientWidth: width, clientHeight: height } = shown.getContainer();
            shownPins = pins.flatMap((pin, i) => {
                const { x, y } = shown.project(at[i]);
                return x >= 0 && x <= width && y >= 0 && y <= height ? [{ ...pin, x, y, index: i, left: x > width / 2 }] : [];
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
    :global(.route-map-mark.ride) {
        border-color: var(--ride);
    }
</style>
