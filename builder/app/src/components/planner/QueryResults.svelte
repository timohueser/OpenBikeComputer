<script lang="ts">
    import PlaceRow from './PlaceRow.svelte';
    import { asPlace } from '../../lib/planner/search/presentation';
    import type { SearchState } from '../../lib/planner/search/types';
    import type { Coordinate, Place } from '../../lib/planner/editor';
    let { state, selectedId, applying = false, applyError = '', onSelect, onApply, onMore, onRetry, onStretch }: {
        state: SearchState; selectedId: string | null; applying?: boolean; applyError?: string;
        onSelect: (place: Place) => void; onApply: () => void; onMore: () => void; onRetry: () => void; onStretch: (line: Coordinate[]) => void;
    } = $props();
    const answer = $derived(state.answer);
</script>
<div class="results" aria-busy={state.loading}>
    {#if state.loading && !answer}<p role="status">Searching…</p>
    {:else if state.error}<p role="alert">{state.error}</p><button type="button" onclick={onRetry}>Retry search</button>
    {:else if answer}
        {#if answer.notice}<p class="note" role="status">{answer.notice}</p>{/if}
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
            {#each answer.stretches ?? [] as stretch}<button type="button" class="stretch" onclick={() => onStretch(stretch.coordinates)}>{stretch.label}<span>km {stretch.from.toFixed(1)}–{stretch.to.toFixed(1)} · {(stretch.to - stretch.from).toFixed(1)} km</span></button>{/each}
            {#if !answer.stretches?.length}<p>No matching stretch in this route data.</p>{/if}
        {:else if answer.type === 'places'}
            <div class="result-heading"><p class="count">{answer.area}</p><span>{answer.results?.length ?? 0}{answer.hasMore ? '+' : ''}</span></div>
            {#each answer.results ?? [] as result (result.source)}
                <PlaceRow wrapDetail place={asPlace(result)} detail={[result.precision === 'street' ? 'Street location only' : '', result.position ? `${result.position.along.toFixed(1)} km along route · ${result.position.distance.toFixed(1)} km off route` : `${result.distance.toFixed(1)} km from search centre`].filter(Boolean).join(' · ')} selected={selectedId === result.source} {onSelect} />
            {:else}<p>No mapped places match this request in this package.</p>{/each}
            {#if answer.hasMore && (answer.results?.length ?? 0) < 100}<button type="button" onclick={onMore}>Show more results</button>{:else if answer.hasMore}<p>Zoom in or narrow the request to see more places.</p>{/if}
        {/if}
        {#if answer.note}<p class="note" role="status">{answer.note}</p>{/if}
        <p class="attribution">© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap contributors</a> · ODbL</p>
    {/if}
</div>
<style>
    .results { padding: 12px 16px 16px; }
    p { margin: 0 0 10px; font-size: 13px; line-height: 1.45; color: var(--ink-soft); overflow-wrap: anywhere; }
    .result-heading { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
    .result-heading p { margin: 0; }
    .result-heading > span { color: var(--ink-soft); font-size: 12px; font-variant-numeric: tabular-nums; }
    .count, .description { color: var(--ink); font-weight: 600; }
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
