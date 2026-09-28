<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    let { place, detail = '', day = null, selected = false, onSelect }: {
        place: Place;
        /** Extra facts after the kind, such as the km mark. */
        detail?: string;
        /** The predicted day if the rider sleeps here. */
        day?: { distance: number; ascent: number; over: boolean } | null;
        selected?: boolean;
        onSelect: (place: Place) => void;
    } = $props();
</script>

<button type="button" class="place-row" class:selected onclick={() => onSelect(place)}>
    <Icon path={placeCategories[place.category].icon} size={17} />
    <span class="name">
        <strong>{place.label}</strong>
        <small>{placeCategories[place.category].label}{detail ? ` · ${detail}` : ''}</small>
    </span>
    {#if day}
        <span class="figure" class:over={day.over}>{day.distance.toFixed(1)} km<small>↑ {day.ascent} m</small></span>
    {/if}
    <Icon name="chevron" size={13} />
</button>

<style>
    .place-row {
        display: flex;
        align-items: center;
        gap: 10px;
        width: calc(100% + 16px);
        margin: 0 -8px;
        padding: 8px;
        border: 0;
        border-radius: 6px;
        background: transparent;
        color: var(--ink);
        text-align: left;
        cursor: pointer;
    }
    .place-row:hover,
    .place-row.selected {
        background: var(--parchment-2);
    }
    .place-row > :global(svg) {
        flex: none;
        color: var(--ink-soft);
    }
    .name {
        flex: 1;
        min-width: 0;
    }
    strong,
    small {
        display: block;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    strong {
        font: 600 14px var(--sans);
    }
    small {
        margin-top: 2px;
        font: 400 13px var(--sans);
        color: var(--ink-soft);
    }
    .figure {
        flex: none;
        text-align: right;
        font: 600 14px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .figure small {
        margin-top: 2px;
    }
    .over {
        color: var(--coral);
    }
</style>
