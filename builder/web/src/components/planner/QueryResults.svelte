<script lang="ts">
    import PlaceRow from './PlaceRow.svelte';
    import { asPlace } from '../../lib/planner/search/presentation';
    import { dayColor } from '../../lib/planner/day-colors';
    import type { SearchPlace, SearchState } from '../../lib/planner/search/types';
    import type { Place } from '../../lib/planner/editor';
    import type { Coordinate } from '../../lib/planner/geo';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    const tenths = (km: number) => distanceIn(km * 1000).toFixed(1);
    let { state, days, theme, selectedId, hoveredId = null, onHover, applying = false, applyError = '', onSelect, onApply, onMore, onRetry, onStretch, attribution }: {
        /** The map data credit of the release. */
        attribution: string;
        /** The riding days in km, with the day numbers of the day list. */
        days: { number: number; from: number; to: number }[]; theme: 'light' | 'dark';
        state: SearchState; selectedId: string | null; hoveredId?: string | null; onHover?: (id: string | null) => void; applying?: boolean; applyError?: string;
        onSelect: (place: Place) => void; onApply: () => void; onMore: () => void; onRetry: () => void; onStretch: (line: Coordinate[]) => void;
    } = $props();
    const answer = $derived(state.answer);
    const results = $derived(answer?.type === 'places' ? answer.results ?? [] : []);
    const showKind = $derived(new Set(results.map(result => result.kind)).size > 1);
    // A place at a day end belongs to the day that ends there.
    const dayIndex = (along: number) => { const index = days.findIndex(day => along <= day.to); return index < 0 ? days.length - 1 : index; };
    const alongs = $derived(results.flatMap(({ position }) => position ? [position.along] : []));
    // A list ranked by anything but route order would repeat its day headings.
    const grouped = $derived(days.length > 1 && alongs.length > 0 && alongs.length === results.length && alongs.every((along, index) => index === 0 || along >= alongs[index - 1]));
    const resultDays = $derived(grouped ? alongs.map(dayIndex) : []);
    const distanceOf = ({ position, distance }: SearchPlace) => position
        ? { main: `${settings.units} ${tenths(position.along)}`, sub: position.distance < 0.05 ? 'on route' : `${tenths(position.distance)} ${settings.units} off` }
        : { main: `${tenths(distance)} ${settings.units}` };
    const count = $derived(answer?.type === 'places' ? `${answer.results?.length ?? 0}${answer.hasMore ? '+' : ''}` : '');
    const summary = $derived(answer?.type === 'places' ? `${count} ${count === '1' ? 'place' : 'places'} found.`
        : answer?.type === 'stretches' ? `${answer.stretches?.length ?? 0} stretches.` : answer?.type === 'change' ? answer.description : '');
    // The one live region of the results; the notice and note show without a role so they are read once.
    const announcement = $derived(state.loading && !answer ? 'Searching…' : [answer?.notice, summary, answer?.note].filter(Boolean).join(' '));
</script>
<div class="results" aria-busy={state.loading}>
    <span class="sr-only" role="status">{announcement}</span>
    {#if state.loading && !answer}<p aria-hidden="true">Searching…</p>
    {/if}
    {#if state.error}<p role="alert">{state.error}</p><button type="button" onclick={onRetry}>Retry search</button>{/if}
    {#if answer}
        {#if answer.notice}<p class="note">{answer.notice}</p>{/if}
        {#if answer.canRetry}<button type="button" onclick={onRetry}>Retry interpretation</button>{/if}
        {#if answer.type === 'change'}
            <p class="description">{answer.description}</p>
            {#each answer.changes ?? [] as change}
                {#if change.point}<p>{change.point.label}{change.point.detail ? ` · ${change.point.detail}` : ''}</p>{/if}
                {#if change.points}<ol>{#each change.points as point}<li>{point.label}{point.detail ? ` · ${point.detail}` : ''}</li>{/each}</ol>{/if}
            {/each}
            <button type="button" class="apply" disabled={applying} onclick={onApply}>{applying ? 'Applying…' : 'Apply change'}</button>
            {#if applyError}<p role="alert">{applyError}</p>{/if}
        {:else if answer.type === 'stretches'}
            <p class="count">{answer.stretches?.length ?? 0} stretches · {answer.area}</p>
            {#each answer.stretches ?? [] as stretch}<button type="button" class="stretch" onclick={() => onStretch(stretch.coordinates)}>{stretch.label}<span>{settings.units} {tenths(stretch.from)}–{tenths(stretch.to)} · {tenths(stretch.to - stretch.from)} {settings.units}</span></button>{/each}
            {#if !answer.stretches?.length}<p>No matching stretch in this route data.</p>{/if}
        {:else if answer.type === 'places'}
            <div class="result-heading"><p class="count">{answer.area}</p><span>{count}</span></div>
            {#each results as result, index (result.source)}
                {#if grouped && resultDays[index] !== resultDays[index - 1]}
                    <h3 class="day-head"><span style:background={dayColor(resultDays[index] + 1, theme)}></span>Day {days[resultDays[index]].number}</h3>
                {/if}
                <PlaceRow place={asPlace(result)} distance={distanceOf(result)} detail={result.precision === 'street' ? 'Street location only' : ''} {showKind} selected={selectedId === result.source} hovered={hoveredId === result.source} {onHover} {onSelect} />
            {:else}<p>No mapped places match this request in this package.</p>{/each}
            {#if answer.hasMore && (answer.results?.length ?? 0) < 100}<button type="button" onclick={onMore}>Show more results</button>{:else if answer.hasMore}<p>Zoom in or narrow the request to see more places.</p>{/if}
        {/if}
        {#if answer.note}<p class="note">{answer.note}</p>{/if}
        <p class="attribution"><a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">{attribution}</a> · ODbL</p>
    {/if}
</div>
<style>
    .results { padding: 12px 16px 16px; }
    .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    p { margin: 0 0 10px; font-size: 13px; line-height: 1.45; color: var(--ink-soft); overflow-wrap: anywhere; }
    .result-heading { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
    .result-heading p { margin: 0; }
    .result-heading > span { color: var(--ink-soft); font-size: 12px; font-variant-numeric: tabular-nums; }
    .count, .description { color: var(--ink); font-weight: 600; }
    /* The negative margin covers the bleed of the rows that scroll under the heading. */
    .day-head { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; gap: 8px; margin: 0 -8px; padding: 9px 8px 5px; background: var(--panel); color: var(--ink-soft); font: 600 11px/1.4 var(--sans); letter-spacing: .06em; text-transform: uppercase; }
    .day-head span { width: 8px; height: 8px; border-radius: 50%; }
    /* A row scrolled into view stays clear of the sticky day heading. */
    .results :global(.place-row) { scroll-margin-top: 30px; }
    .results :global(.place-row + .place-row) { box-shadow: 0 -1px 0 var(--line); }
    .results :global(.place-row:is(:hover, .selected, .hovered)), .results :global(.place-row:is(:hover, .selected, .hovered) + .place-row) { box-shadow: none; }
    .note { margin-top: 12px; }
    ol { padding-inline-start: 22px; font-size: 13px; }
    button { min-height: 36px; padding: 7px 10px; border: 1px solid var(--line); border-radius: 6px; background: var(--panel); color: var(--ink); font: inherit; font-size: 13px; cursor: pointer; }
    button:hover { border-color: var(--ink-soft); }
    .apply { background: var(--amber); color: var(--black, #171717); margin-block: 10px; }
    button:disabled { cursor: wait; opacity: .6; }
    button:focus-visible, a:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .stretch { width: 100%; text-align: start; margin-block: 4px; }
    .stretch span { display: block; color: var(--ink-soft); margin-top: 4px; }
    .attribution { font-size: 11px; margin-top: 16px; }
    a { color: inherit; text-underline-offset: 2px; }
</style>
