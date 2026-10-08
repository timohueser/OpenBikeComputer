<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import OpeningStatus from './OpeningStatus.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    let { place, detail = '', day = null, selected = false, wrapDetail = false, hovered = false, onHover, onSelect }: {
        place: Place;
        /** Extra facts after the kind, such as the km mark. */
        detail?: string;
        /** The predicted day if the rider sleeps here. */
        day?: { distance: number; ascent: number; over: boolean } | null;
        selected?: boolean;
        wrapDetail?: boolean;
        hovered?: boolean;
        onHover?: (id: string | null) => void;
        onSelect: (place: Place) => void;
    } = $props();
</script>

<button type="button" class="place-row" class:selected class:wrapDetail class:hovered onmouseenter={() => onHover?.(place.id)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(place.id)} onblur={() => onHover?.(null)} onclick={() => onSelect(place)}>
    <span class="place-icon"><Icon path={placeCategories[place.category].icon} size={17} /></span>
    <span class="name">
        <strong>{place.label}</strong>
        <small>{place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label}{place.locality ? ` · ${place.locality}` : ''}</small>
        {#if detail}<span class="detail">{detail}</span>{/if}
        {#if wrapDetail && place.hoursStatus}<OpeningStatus value={place.hoursStatus} />{/if}
    </span>
    {#if day}
        <span class="figure" class:over={day.over}>≈ {day.distance.toFixed(1)} km<small>↑ {day.ascent} m</small></span>
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
    .place-row.selected,
    .place-row.hovered {
        background: var(--parchment-2);
    }
    .place-row:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
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
    .place-icon { display: grid; place-items: center; flex: none; color: var(--ink-soft); }
    .detail { display: block; margin-top: 4px; color: var(--ink-soft); font: 400 12px/1.5 var(--sans); font-variant-numeric: tabular-nums; }
    .wrapDetail { align-items: flex-start; padding-block: 14px; border-radius: 8px; }
    .wrapDetail .place-icon { width: 30px; height: 30px; border-radius: 50%; background: color-mix(in srgb, var(--query-place) 10%, var(--panel)); color: var(--query-place); }
    .wrapDetail > :global(svg) { margin-top: 8px; }
    .wrapDetail strong { white-space: normal; overflow-wrap: anywhere; line-height: 1.4; }
    .wrapDetail .name small { white-space: normal; overflow-wrap: anywhere; line-height: 1.4; }
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
