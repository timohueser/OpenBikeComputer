<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import OpeningStatus from './OpeningStatus.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import type { Place } from '../../lib/planner/editor';

    let { place, distance, detail = '', showKind = true, selected = false, hovered = false, onHover, onSelect }: {
        place: Place;
        /** The right column, such as the km mark over the distance off the route. */
        distance: { main: string; sub?: string };
        /** Extra facts after the locality, such as "Street location only". */
        detail?: string;
        showKind?: boolean;
        selected?: boolean;
        hovered?: boolean;
        onHover?: (id: string | null) => void;
        onSelect: (place: Place) => void;
    } = $props();
    const facts = $derived([showKind && (place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label), place.locality, detail].filter(Boolean).join(' · '));
</script>

<button type="button" class="place-row" class:selected class:hovered onmouseenter={() => onHover?.(place.id)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(place.id)} onblur={() => onHover?.(null)} onclick={() => onSelect(place)}
    {@attach row => { if (hovered && !row.matches(':hover')) row.scrollIntoView({ block: 'nearest', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' }); }}>
    <span class="place-icon"><Icon path={placeCategories[place.category].icon} size={17} /></span>
    <span class="name">
        <strong>{place.label}</strong>
        {#if facts}<small>{facts}</small>{/if}
        {#if place.hoursStatus}<OpeningStatus value={place.hoursStatus} />{/if}
    </span>
    <span class="distance"><b>{distance.main}</b>{#if distance.sub}<span>{distance.sub}</span>{/if}</span>
</button>

<style>
    .place-row {
        display: flex;
        align-items: flex-start;
        gap: 10px;
        width: calc(100% + 16px);
        margin: 0 -8px;
        padding: 8px;
        border: 0;
        border-radius: 8px;
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
    .place-row:hover .place-icon, .selected .place-icon, .hovered .place-icon { box-shadow: 0 0 0 2px var(--amber); }
    .place-row:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .name {
        flex: 1;
        min-width: 0;
    }
    strong,
    small {
        display: block;
        overflow-wrap: anywhere;
    }
    strong {
        font: 600 14px/1.4 var(--sans);
    }
    small {
        margin-top: 2px;
        font: 400 13px/1.4 var(--sans);
        color: var(--ink-soft);
    }
    .place-icon { display: grid; place-items: center; flex: none; width: 30px; height: 30px; border-radius: 50%; background: color-mix(in srgb, var(--query-place) 10%, var(--panel)); color: var(--query-place); }
    .distance { flex: none; text-align: right; font-variant-numeric: tabular-nums; }
    .distance b { display: block; font: 600 13px/1.5 var(--sans); }
    .distance span { display: block; margin-top: 2px; color: var(--ink-soft); font: 400 12px/1.5 var(--sans); }
</style>
