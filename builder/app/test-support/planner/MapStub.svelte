<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Coordinate } from '../../src/lib/planner/editor';
    import type { MapPoint } from '../../src/lib/planner/map-types';

    let { popup, points = [], onPointSelect, onEmptyClick }: { popup?: Snippet; points?: MapPoint[]; onPointSelect?: (id: string) => void; onEmptyClick?: (coordinate: Coordinate) => void } = $props();
    export function fitSearchResults() {}
    export function fitRoute() {}
    export function fitCoordinates() {}
    export function showPlace() {}
    export function zoomBy() {}
</script>

<button type="button" onclick={() => onEmptyClick?.([7.84, 48])}>Pick map location</button>
{#each points as point (point.id)}
    <button type="button" aria-label={`Map ${point.kind === 'place' ? 'place' : 'point'}: ${point.label}`} onclick={() => onPointSelect?.(point.id)}>{point.label}</button>
{/each}
{@render popup?.()}
