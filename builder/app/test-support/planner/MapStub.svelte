<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { MapPoint } from '../../src/lib/planner/map-types';

    let { popup, points = [], onPointSelect }: { popup?: Snippet; points?: MapPoint[]; onPointSelect?: (id: string) => void } = $props();
    export function fitSearchResults() {}
    export function fitRoute() {}
    export function fitCoordinates() {}
    export function showPlace() {}
    export function zoomBy() {}
</script>

{#each points.filter(point => point.kind === 'place') as point (point.id)}
    <button type="button" aria-label={`Map place: ${point.label}`} onclick={() => onPointSelect?.(point.id)}>{point.label}</button>
{/each}
{@render popup?.()}
