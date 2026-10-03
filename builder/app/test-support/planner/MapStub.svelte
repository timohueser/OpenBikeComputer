<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Coordinate } from '../../src/lib/planner/editor';
    import type { MapPoint } from '../../src/lib/planner/map-types';

    let { accessMode = 'cycling', popup, points = [], onPointSelect, onPointHover, hoveredId, onEmptyClick, onBounds }: { accessMode?: 'cycling' | 'walking'; onBounds?: (bounds: [number, number, number, number], preserveSearch: boolean) => void; hoveredId?: string | null; onPointHover?: (id: string | null) => void; popup?: Snippet; points?: MapPoint[]; onPointSelect?: (id: string) => void; onEmptyClick?: (coordinate: Coordinate) => void } = $props();
    export function fitSearchResults() {}
    export function fitRoute() {}
    export function fitCoordinates() {}
    export function showPlace() {}
    export function zoomBy() {}
    export function centerOn() {}
    export function underTrees() { return false; }
</script>

<button type="button" data-access-mode={accessMode} onclick={() => onBounds?.([7.9, 48, 8, 48.1], false)}>Pan map</button>
<button type="button" onclick={() => onEmptyClick?.([7.84, 48])}>Pick map location</button>
{#each points as point (point.id)}
    <button type="button" aria-label={`Map ${point.kind === 'place' ? 'place' : 'point'}: ${point.label}`} class:highlighted={hoveredId === point.id} onmouseenter={() => onPointHover?.(point.id)} onmouseleave={() => onPointHover?.(null)} onclick={() => onPointSelect?.(point.id)}>{point.label}</button>
{/each}
{@render popup?.()}
