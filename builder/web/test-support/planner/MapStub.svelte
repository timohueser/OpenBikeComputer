<script module lang="ts">
    import type { Coordinate } from '../../src/lib/planner/geo';

    /** The town names of the loaded map tiles; a test spies on it. */
    export const tiles = { placeName: (_coordinate: Coordinate): string | undefined => undefined };
    /** The arguments of each `showNight` call. */
    export const nightCalls: [Coordinate, Coordinate[][]][] = [];
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { MapPoint } from '../../src/lib/planner/map-types';

    let { accessMode = 'cycling', popup, points = [], onPointSelect, onPointMove, onPointHover, hoveredId, onEmptyClick, onBounds }: { accessMode?: 'cycling' | 'walking'; onPointMove?: (id: string, coordinate: Coordinate) => void; onBounds?: (bounds: [number, number, number, number], preserveSearch: boolean) => void; hoveredId?: string | null; onPointHover?: (id: string | null) => void; popup?: Snippet; points?: MapPoint[]; onPointSelect?: (id: string) => void; onEmptyClick?: (coordinate: Coordinate) => void } = $props();
    export function fitSearchResults() {}
    export function fitRoute() {}
    export function fitCoordinates() {}
    export function showNight(spot: Coordinate, towns: Coordinate[][]) { nightCalls.push([spot, towns]); }
    export function showPlace() {}
    export function zoomBy() {}
    export function centerOn() {}
    export function underTrees() { return false; }
    export function placeName(coordinate: Coordinate) { return tiles.placeName(coordinate); }
</script>

<button type="button" data-access-mode={accessMode} onclick={() => onBounds?.([7.9, 48, 8, 48.1], false)}>Pan map</button>
<button type="button" onclick={() => onEmptyClick?.([7.84, 48])}>Pick map location</button>
<button type="button" onclick={() => onPointMove?.('start', [7.61, 47.55])}>Move start</button>
{#each points as point (point.id)}
    <button type="button" aria-label={`Map ${point.kind === 'place' ? 'place' : 'point'}: ${point.label}`} class:highlighted={hoveredId === point.id} onmouseenter={() => onPointHover?.(point.id)} onmouseleave={() => onPointHover?.(null)} onclick={() => onPointSelect?.(point.id)}>{point.label}</button>
{/each}
{@render popup?.()}
