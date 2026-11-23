<!-- The answer of the planner search: the places with the action of "Find" on each, the stretches, or one action row for a
     change of the plan that the slots cannot hold. After an action, one line says what it changed and has "Undo". -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import { asPlace } from '../../lib/planner/search/presentation';
    import { dayColor } from '../../lib/planner/day-colors';
    import type { SearchPlace, SearchState } from '../../lib/planner/search/types';
    import type { Place } from '../../lib/planner/editor';
    import { distanceIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import { rise, still, unfold } from '../../lib/ui/search-bar/motion';
    const tenths = (km: number) => distanceIn(km * 1000).toFixed(1);
    let { state, days, theme, selectedId, hoveredId = null, onHover, applying = false, applyError = '', action, done = '', onSelect, onApply, onAct, onUndo, onMore, onRetry, onStretch, attribution }: {
        /** The map data credit of the release. */
        attribution: string;
        /** The riding days in km, with the day numbers of the day list. */
        days: { number: number; from: number; to: number }[]; theme: 'light' | 'dark';
        state: SearchState; selectedId: string | null; hoveredId?: string | null; onHover?: (id: string | null) => void; applying?: boolean; applyError?: string;
        /** The button of the action on each place. When the rider chose the action, it is amber on the first place. */
        action?: { button: string; chosen: boolean };
        /** What the last action changed. */
        done?: string;
        onSelect: (place: Place) => void; onApply: () => void; onAct?: (place: SearchPlace) => void; onUndo?: () => void; onMore: () => void; onRetry: () => void;
        /** A stretch of the list, from and to in km of the route. */
        onStretch: (stretch: [from: number, to: number]) => void;
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
    /** The size of the list with its noun, such as "3 places", "20+ places" when the server has more, or "1 stretch". */
    const count = $derived.by(() => {
        if (answer?.type === 'stretches') return answer.stretches?.length === 1 ? '1 stretch' : `${answer.stretches?.length ?? 0} stretches`;
        if (answer?.type !== 'places') return '';
        return results.length === 1 && !answer.hasMore ? '1 place' : `${results.length}${answer.hasMore ? '+' : ''} places`;
    });
    const summary = $derived(answer?.type === 'change' ? answer.description : count && `${count} found.`);
    const points = $derived((answer?.changes ?? []).flatMap(change => change.points ?? (change.point ? [change.point] : []))
        .map(point => point.detail ? `${point.label}, ${point.detail}` : point.label).join(' · '));
    // The one live region of the results; the notice, the line after an action and the note show without a role so they
    // are read once.
    const announcement = $derived(state.loading && !answer ? 'Searching…' : [answer?.notice, done, summary, answer?.note].filter(Boolean).join(' '));
    /** A new answer shows as a short fade of the list, as on Explore. */
    function fade(list: HTMLElement) {
        void answer;
        if (!still()) list.animate?.({ opacity: [0.55, 1] }, { duration: 180, easing: 'ease-out' });
    }
</script>
<!-- The line over a list: its size, and the area that the search used, which an empty list shows too. -->
{#snippet heading(size: number)}
    {#if size || answer?.area}
        <div class="result-heading">{#if size}<h2>{#key count}<span in:rise>{count}</span>{/key}</h2>{/if}{#if answer?.area}<span>{answer.area}</span>{/if}</div>
    {/if}
{/snippet}
<div class="results" aria-busy={state.loading} {@attach fade}>
    <span class="sr-only" role="status">{announcement}</span>
    {#if state.loading && !answer}<p aria-hidden="true">Searching…</p>
    {/if}
    {#if state.error}<p role="alert">{state.error}</p><button type="button" onclick={onRetry}>Retry search</button>{/if}
    {#if answer}
        {#if answer.notice}<p>{answer.notice}</p>{/if}
        {#if answer.canRetry}<button type="button" onclick={onRetry}>Retry interpretation</button>{/if}
        {#if answer.type === 'change'}
            <div class="action-row">
                <span class="action-icon"><Icon name="route" size={17} /></span>
                <span class="action-text"><strong>{answer.description}</strong>{#if points}<small>{points}</small>{/if}</span>
                <button type="button" class="act primary" disabled={applying} onclick={onApply}>{applying ? 'Applying…' : 'Apply'}</button>
            </div>
            {#if applyError}<p role="alert">{applyError}</p>{/if}
        {:else if answer.type === 'stretches'}
            {@render heading(answer.stretches?.length ?? 0)}
            {#each answer.stretches ?? [] as stretch}<button type="button" class="stretch" onclick={() => onStretch([stretch.from, stretch.to])}>{stretch.label}<span>{settings.units} {tenths(stretch.from)}–{tenths(stretch.to)} · {tenths(stretch.to - stretch.from)} {settings.units}</span></button>{/each}
            {#if !answer.stretches?.length}<p>No matching stretch in this route data.</p>{/if}
        {:else if answer.type === 'places'}
            {@render heading(results.length)}
            {#if done}<p class="done" in:unfold><Icon name="check" size={16} /><span>{done}</span><button type="button" class="act" onclick={onUndo}>Undo</button></p>{/if}
            {#if applyError}<p role="alert">{applyError}</p>{/if}
            {#each results as result, index (result.source)}
                {#if grouped && resultDays[index] !== resultDays[index - 1]}
                    <h3 class="day-head"><span style:background={dayColor(resultDays[index] + 1, theme)}></span>Day {days[resultDays[index]].number}</h3>
                {/if}
                <div class="result">
                    <PlaceRow place={asPlace(result)} distance={distanceOf(result)} detail={result.precision === 'street' ? 'Street location only' : ''} {showKind} selected={selectedId === result.source} hovered={hoveredId === result.source} {onHover} {onSelect} />
                    {#if action}<button type="button" class="act" class:primary={action.chosen && index === 0} disabled={applying} onclick={() => onAct?.(result)}>{action.button}</button>{/if}
                </div>
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
    /* A search that takes long dims the list that it replaces. */
    .results { transition: opacity 160ms; }
    .results[aria-busy="true"] { opacity: .55; transition-delay: 240ms; }
    .result-heading { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: 2px 12px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
    h2 { margin: 0; color: var(--ink); font: 700 15px/1.4 var(--sans); font-variant-numeric: tabular-nums; }
    /* A box, so the new count can rise into its place. */
    h2 span { display: inline-block; }
    .result-heading > span { color: var(--ink-soft); font-size: 13px; }
    /* The negative margin covers the bleed of the rows that scroll under the heading. */
    .day-head { position: sticky; top: 0; z-index: 1; display: flex; align-items: center; gap: 8px; margin: 0 -8px; padding: 9px 8px 5px; background: var(--panel); color: var(--ink-soft); font: 600 11px/1.4 var(--sans); letter-spacing: .06em; text-transform: uppercase; }
    .day-head span { width: 8px; height: 8px; border-radius: 50%; }
    /* A row scrolled into view stays clear of the sticky day heading. */
    .results :global(.place-row) { scroll-margin-top: 30px; }
    .result { display: flex; align-items: center; gap: 8px; }
    .result > :global(.place-row) { flex: 1 1 0; min-width: 0; margin-right: 0; }
    .result + .result { box-shadow: 0 -1px 0 var(--line); }
    .result:has(> :global(.place-row:is(:hover, .selected, .hovered))), .result:has(> :global(.place-row:is(:hover, .selected, .hovered))) + .result { box-shadow: none; }
    .note { margin-top: 12px; }
    button { display: block; min-height: 36px; margin-block: 10px; padding: 7px 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font: inherit; font-size: 13px; cursor: pointer; transition: background 120ms, border-color 120ms; }
    button:hover:not(:disabled) { border-color: var(--ink-faint); background: var(--parchment-2); }
    /* The action on a place, and "Apply" and "Undo": the one amber button is the action that the rider chose. */
    .act { flex: none; min-height: 30px; margin: 0; padding: 0 10px; font-weight: 600; font-size: 12.5px; white-space: nowrap; }
    .act.primary { border-color: var(--amber); background: var(--amber); color: var(--on-amber); }
    .act.primary:hover:not(:disabled) { border-color: transparent; background: color-mix(in srgb, var(--amber) 86%, var(--ink)); }
    .done { display: flex; align-items: center; gap: 8px; margin: 10px 0 4px; padding: 6px 6px 6px 12px; border-radius: 8px; background: var(--parchment-2); color: var(--ink); font-weight: 600; }
    .done span { flex: 1; min-width: 0; }
    .done > :global(svg) { flex: none; color: var(--ink-soft); }
    .action-row { display: flex; align-items: center; gap: 10px; margin-bottom: 10px; }
    .action-icon { display: grid; place-items: center; flex: none; width: 30px; height: 30px; border-radius: 50%; background: var(--parchment-2); color: var(--ink-soft); }
    .action-text { flex: 1; min-width: 0; overflow-wrap: anywhere; }
    .action-text strong, .action-text small { display: block; }
    .action-text strong { font: 600 14px/1.4 var(--sans); }
    .action-text small { margin-top: 2px; color: var(--ink-soft); font: 400 13px/1.4 var(--sans); }
    button:disabled { cursor: wait; opacity: .6; }
    button:focus-visible, a:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .stretch { width: 100%; text-align: start; margin-block: 6px; }
    .result-heading + .stretch { margin-top: 10px; }
    .stretch span { display: block; color: var(--ink-soft); margin-top: 4px; }
    .attribution { font-size: 11px; margin-top: 16px; }
    a { color: inherit; text-underline-offset: 2px; }
</style>
