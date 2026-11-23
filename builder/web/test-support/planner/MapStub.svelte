<script module lang="ts">
    import type { Coordinate } from '../../src/lib/planner/geo';

    /** The town names of the loaded map tiles; a test spies on it. */
    export const tiles = { placeName: (_coordinate: Coordinate): string | undefined => undefined };
    /** The arguments of each `showNight` call. */
    export const nightCalls: [Coordinate, Coordinate[][]][] = [];
    /** How often the view moved to a route, coordinates or a place. */
    export const moves = { count: 0 };
    /** How often the page asked the map to show the week. */
    export const weekFits = { count: 0 };
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { MapPoint, MapTown } from '../../src/lib/planner/map-types';
    import type { Week } from '../../src/lib/planner/week';

    let { accessMode = 'cycling', popup, points = [], towns = [], litStop = null, week = null, lit = null, onStopHover, onStopOpen, onPointSelect, onPointMove, onPointHover, hoveredId, onEmptyClick, onBounds }: { accessMode?: 'cycling' | 'walking'; onPointMove?: (id: string, coordinate: Coordinate) => void; onBounds?: (bounds: [number, number, number, number], preserveSearch: boolean) => void; hoveredId?: string | null; onPointHover?: (id: string | null) => void; popup?: Snippet; points?: MapPoint[]; towns?: MapTown[]; litStop?: string | null; onStopHover?: (id: string | null) => void; onStopOpen?: (id: string) => void; week?: Week | null; lit?: number | null; onPointSelect?: (id: string) => void; onEmptyClick?: (coordinate: Coordinate) => void } = $props();
    export function fitSearchResults() {}
    export function fitRoute() { moves.count++; }
    export function fitCoordinates() { moves.count++; }
    export function showStop() { moves.count++; }
    export function showNight(spot: Coordinate, towns: Coordinate[][]) { nightCalls.push([spot, towns]); }
    export function showWeek() { weekFits.count++; }
    export function showPlace() { moves.count++; }
    export function zoomBy() {}
    export function underTrees() { return false; }
    export function placeName(coordinate: Coordinate) { return tiles.placeName(coordinate); }
</script>

<button type="button" data-access-mode={accessMode} onclick={() => onBounds?.([7.9, 48, 8, 48.1], false)}>Pan map</button>
<button type="button" onclick={() => onEmptyClick?.([7.84, 48])}>Pick map location</button>
<button type="button" onclick={() => onPointMove?.('start', [7.61, 47.55])}>Move start</button>
{#each points as point (point.id)}
    <button type="button" aria-label={`Map ${point.kind === 'place' ? 'place' : 'point'}: ${point.label}`} class:highlighted={hoveredId === point.id} class:lone={!!point.stop} class:lit={!!point.stop && point.stop === litStop}
        onmouseenter={() => { onPointHover?.(point.id); if (point.stop) onStopHover?.(point.stop); }} onmouseleave={() => { onPointHover?.(null); if (point.stop) onStopHover?.(null); }} onclick={() => onPointSelect?.(point.id)}>{point.label}</button>
{/each}
{#each towns as town (town.id)}
    <button type="button" aria-label={`Map town: ${town.name}`} class:lit={town.id === litStop} class:open={town.open} onmouseenter={() => onStopHover?.(town.id)} onmouseleave={() => onStopHover?.(null)} onclick={() => onStopOpen?.(town.id)}>{town.places.length}</button>
{/each}
<output class="map-week">{week ? `${week.start}-${week.end}` : ''} {lit ?? ''}</output>
{@render popup?.()}
