<script lang="ts">
    import { onMount } from 'svelte';
    import * as maplibregl from 'maplibre-gl';
    import 'maplibre-gl/dist/maplibre-gl.css';
    import type { Summary } from '../../lib/library/records';
    import { lineCoordinates } from '../../lib/library/summary';
    import { coordinateAt, nearestOnLine, nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import Icon from '../planner/PlannerIcon.svelte';
    import { basemapStyle } from '../../lib/planner/map-style';
    import { styleConfig, useMapProtocols } from '../../lib/planner/map-setup';
    import { appearance } from '../../lib/theme.svelte';

    /** A map that the rider can move, with the summary line of a route or a ride and the mark. A share is 0 to 1 along the line. */
    let { summary, ride = false, mark, pins = [], onpoint }: {
        summary: Summary;
        /** The line is a ride, in `--ride`. */
        ride?: boolean;
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
    let shownPins = $state.raw<{ share: number; label: string; number?: number; x: number; y: number }[]>([]);
    /** The pin whose name shows, by its index; one at a time. */
    let opened = $state<number>();
    const pinName = (pin: { label: string; number?: number }) => pin.number ? pin.label : pin.label ? `Night · ${pin.label}` : 'Night';
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
            paint: { 'line-color': style.getPropertyValue(ride ? '--ride' : '--route').trim(), 'line-width': 4 } });
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
                return x >= 0 && x <= width && y >= 0 && y <= height ? [{ ...pin, x, y }] : [];
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
        {#each shownPins as pin, i (i)}
            <span class="route-pin" style:left={`${pin.x}px`} style:top={`${pin.y}px`}>
                <button type="button" class:night={!pin.number} aria-label={pinName(pin)} aria-expanded={opened === i}
                    onclick={() => opened = i} onfocus={() => opened = i} onblur={() => opened = undefined}
                    onkeydown={event => { if (event.key === 'Escape') opened = undefined; }}>
                    {#if pin.number}{pin.number}{:else}<Icon name="moon" size={12} />{/if}
                </button>
                {#if opened === i}<span class="route-pin-label">{pinName(pin)}</span>{/if}
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
