<script lang="ts">
    import PlaceRow from './PlaceRow.svelte';
    import type { Place } from '../../lib/planner/editor';

    let { results, selectedId, onSelect }: {
        /** `km` is the mark along the route; `off` the straight distance from it. */
        results: { place: Place; km: number; off: number }[];
        selectedId: string | null;
        onSelect: (place: Place) => void;
    } = $props();
</script>

<div class="results">
    <p class="count">{results.length} {results.length === 1 ? 'place' : 'places'}</p>
    {#each results as { place, km, off } (place.id)}
        <PlaceRow {place} detail={`km ${km.toFixed(1)} · +${off.toFixed(1)} km off route`} selected={selectedId === place.id} {onSelect} />
    {:else}
        <p class="empty">Nothing found along the route for that.</p>
    {/each}
    <p class="note">Example places · no live availability</p>
</div>

<style>
    .results {
        padding: 12px 16px 16px;
    }
    p {
        margin: 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .count {
        margin-bottom: 4px;
        font-weight: 600;
        color: var(--ink);
    }
    .empty {
        padding: 8px 0;
    }
    .note {
        margin-top: 12px;
    }
</style>
