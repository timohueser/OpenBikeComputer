<script lang="ts">
    import PlaceRow from './PlaceRow.svelte';
    import type { Place } from '../../lib/planner/editor';

    let { results, days, searchedDay, loading = false, selectedId, onSelect }: {
        /** `where` places the result along the trip; `off` is the straight distance from the route in km. */
        results: { place: Place; where: string; off: number }[];
        /** The riding days as calendar numbers with their colours; empty for a single route. */
        days: { number: number; color: string }[];
        searchedDay: number | null;
        loading?: boolean;
        selectedId: string | null;
        onSelect: (place: Place) => void;
    } = $props();
</script>

<div class="results">
    {#if days.length}
        <ol class="days" aria-label="Days">
            {#each days as day (day.number)}
                <li class:current={day.number === searchedDay} style:--day-color={day.color}><span class="badge">{day.number}</span>Day {day.number}</li>
            {/each}
        </ol>
    {/if}
    <p class="count">{loading ? 'Looking along the route…' : `${results.length} ${results.length === 1 ? 'place' : 'places'}`}</p>
    {#each results as { place, where, off } (place.id)}
        <PlaceRow {place} detail={off >= .1 ? `${where} · ${off.toFixed(1)} km off route` : where} selected={selectedId === place.id} {onSelect} />
    {:else}
        {#if !loading}<p class="empty">Nothing found along the route for that.</p>{/if}
    {/each}
    <p class="note">Example places · no live availability</p>
</div>

<style>
    .results {
        padding: 12px 16px 16px;
    }
    .days {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 12px;
        margin: 0 0 12px;
        padding: 0 0 12px;
        list-style: none;
        border-bottom: 1px solid var(--line);
        font-size: 13px;
        color: var(--ink-soft);
    }
    .days li {
        display: flex;
        align-items: center;
        gap: 6px;
    }
    .days .current {
        color: var(--ink);
        font-weight: 700;
    }
    .badge {
        display: grid;
        place-items: center;
        width: 20px;
        height: 20px;
        border-radius: 50%;
        background: var(--day-color);
        color: var(--panel);
        font: 700 11px var(--sans);
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
