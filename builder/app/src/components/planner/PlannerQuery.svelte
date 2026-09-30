<script lang="ts">
    import { onDestroy, onMount, untrack } from 'svelte';
    import PlannerIcon from './PlannerIcon.svelte';
    import { HOSTED_SEARCH } from '../../lib/planner/search/config';
    import QueryChip from './QueryChip.svelte';
    import type { Coordinate } from '../../lib/planner/editor';
    import { searchPlaces, type QueryRequest, type SearchContext, type SearchState, type Where } from '../../lib/planner/search/types';

    let { text = $bindable(''), searchState = $bindable({ loading: false, error: '', answer: null }), context, selection, region = $bindable('baden-wuerttemberg'), viewRevision = 0, onResults, onSearch, onClear, onLocation, onSample, onDate, onPointing }: {
        text?: string; region?: string; searchState?: SearchState; context: SearchContext; selection?: Where;
        viewRevision?: number; onResults?: (coordinates: Coordinate[]) => void; onSearch: () => void; onClear: () => void; onLocation: () => void; onSample: () => void; onDate: (date: string) => void; onPointing: (where?: Where) => void;
    } = $props();
    let edited = $state(false);
    let request = $state<QueryRequest | undefined>();
    let removed = $state<Record<string, unknown>>({});
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let serial = 0;
    let previousContext = '';
    let framePending = false;
    let requestContext: SearchContext | undefined;
    let limit = 6;
    let settings = $state(false);
    const regions = (import.meta.env.VITE_PLANNER_SEARCH_REGIONS || 'baden-wuerttemberg,germany').split(',');
    const regionName = (id: string) => id === 'germany' ? 'Germany' : id === 'baden-wuerttemberg' ? 'Baden-Württemberg' : id.replaceAll('-', ' ');
    let activeFilter = $state<string | null>(null);
    const days = $derived(context.plan.days.filter(d => !d.rest).map(d => d.number));
    const fields = $derived(Object.entries(request ?? {}).filter(([key]) => !['type','ignored','via_source'].includes(key)));
    const required = $derived(request?.type === 'places' ? ['what'] : request?.type === 'place' ? ['name'] : request?.type === 'route' ? ['to'] : request?.type === 'end_day' ? ['day','at'] : ['add_point','remove_point'].includes(request?.type ?? '') ? ['point'] : request?.type === 'split' ? ['days','per_day'] : request?.type === 'join' ? ['day'] : request?.type === 'stretches' ? ['what'] : []);
    const explicitWhere = $derived(request?.where ?? context.pointing ?? { scope: 'view' as const });

    async function run(nextLimit = 20, parsed = request, options: { reparse?: boolean; fit?: boolean; background?: boolean; context?: SearchContext } = {}) {
        const { reparse = false, fit = true, background = false } = options;
        if (fit) framePending = true;
        if (reparse) parsed = undefined;
        clearTimeout(timer); controller?.abort(); const id = ++serial;
        if (!text.trim()) { clear(); return; }
        controller = new AbortController(); limit = nextLimit;
        const input = text, signal = controller.signal;
        const searchContext = options.context ?? (background && requestContext ? requestContext : $state.snapshot(context));
        requestContext = searchContext;
        if (!background) onSearch();
        searchState = { loading: true, error: '', answer: background ? searchState.answer : null };
        try {
            const answer = await searchPlaces(input, searchContext, region, limit, signal, parsed ? $state.snapshot(parsed) : undefined);
            if (id !== serial) return;
            request = answer.request; searchState = { loading: false, error: '', answer };
            if (framePending && answer.type === 'places' && answer.results?.length) onResults?.(answer.results.map(p => [p.lon, p.lat]));
            framePending = false;
        } catch (error) {
            if (id !== serial || signal.aborted) return;
            if (background) { searchState = { ...searchState, loading: false }; return; }
            searchState = { loading: false, answer: null, error: error instanceof TypeError ? 'Search is unavailable. Check your connection and retry.' : (error as Error).message };
        }
    }
    function input(value: string) {
        framePending = true;
        text = value; request = undefined; removed = {}; edited = false; activeFilter = null;
        clearTimeout(timer); controller?.abort(); serial++;
        searchState = { loading: !!text.trim(), error: '', answer: null };
        if (!text.trim()) { onClear(); return; }
        onSearch(); timer = setTimeout(() => run(6, undefined), 350);
    }
    function clear() {
        clearTimeout(timer); controller?.abort(); serial++;
        framePending = false;
        text = ''; request = undefined; removed = {}; edited = false; activeFilter = null;
        searchState = { loading: false, error: '', answer: null }; onClear();
    }
    function edit(key: string, value: unknown) {
        if (!request) return;
        const next = { ...request, [key]: value };
        const absent = { ...removed }; delete absent[key]; removed = absent;
        request = next; edited = true; run(20, next);
    }
    function toggle(key: string, value: unknown) {
        if (key in removed) { edit(key, removed[key]); return; }
        const next = { ...request } as QueryRequest;
        delete (next as unknown as Record<string, unknown>)[key];
        removed = { ...removed, [key]: value }; request = next; edited = true; run(20, next);
    }
    export function more() { run(Math.min(100, Math.max(20, limit + 20)), request, { context: requestContext }); }
    export function retry() { run(Math.max(20, limit), request, { reparse: !edited, context: requestContext }); }
    $effect(() => {
        // Result framing and place inspection update live bounds without advancing viewRevision.
        const current = JSON.stringify([viewRevision, context.here, context.startDate, context.pointing, context.plan]);
        if (current === previousContext) return;
        previousContext = current;
        untrack(() => { if (text.trim() && current) { clearTimeout(timer); controller?.abort(); serial++; searchState = { loading: true, error: '', answer: null }; timer = setTimeout(() => run(limit, request, { fit: false }), 200); } });
    });
    onMount(() => {
        const refresh = setInterval(() => {
            if (!document.hidden && !searchState.loading && searchState.answer?.results?.some(p => p.opening_hours)) run(limit, request, { fit: false, background: true });
        }, 60_000);
        return () => clearInterval(refresh);
    });
    onDestroy(() => { clearTimeout(timer); controller?.abort(); });
</script>
<div class="planner-query">
    <form onsubmit={event => { event.preventDefault(); run(20, request, { reparse: !edited }); }}>
        <div class="query-input" class:edited>
            <PlannerIcon name="search" size={17} />
            <input aria-label="Find a place or ask about the route" value={text} oninput={e => input(e.currentTarget.value)} placeholder="Find a place, or ask along your route…" maxlength="240" onkeydown={e => { if (e.key === 'Escape') clear(); }} />
            {#if text}<button type="button" aria-label="Clear search" class="clear" onclick={clear}><PlannerIcon name="close" size={16} /></button>{/if}
        </div>
    </form>
    {#if text.trim()}
        {#if request}
            <div class="meaning" aria-label="Understood request">
                <span class="meaning-label">{edited ? 'Edited request' : request.type === 'place' ? 'Place search' : request.type.replaceAll('_', ' ')}</span>
                {#each fields as [field, value] (field)}
                    <QueryChip bind:active={activeFilter} {field} {value} {days} {selection} removable={!required.includes(field)} onChange={value => edit(field, value)} onToggle={() => toggle(field, value)} />
                {/each}
                {#if request.type === 'places' && !request.where && !('where' in removed)}
                    <QueryChip bind:active={activeFilter} field="where" value={explicitWhere} {days} {selection} removable={false} onChange={value => edit('where', value)} onToggle={() => {}} />
                {/if}
                {#each Object.entries(removed) as [field, value] (field)}<QueryChip bind:active={activeFilter} {field} {value} {days} {selection} removed onChange={value => edit(field, value)} onToggle={() => toggle(field, value)} />{/each}
            </div>
            {#if request.ignored?.length}<p class="note" role="status">Not understood: {request.ignored.map(word => `“${word}”`).join(', ')}. These words are ignored.</p>{/if}
        {/if}
        {#if text.length > 80}<p class="note">Smart requests use up to 80 characters. Longer text uses ordinary place search.</p>{/if}
    {/if}
    {#if !text.trim()}
        <div class="meaning"><QueryChip bind:active={activeFilter} field="where" value={context.pointing ?? { scope: 'view' }} {days} {selection} onChange={value => onPointing(value as Where)} onToggle={() => onPointing()} /></div>
    {/if}
    <button type="button" class="data-button" aria-expanded={settings} onclick={() => settings = !settings}>{regionName(region)} · {HOSTED_SEARCH ? 'online' : 'local data'}<PlannerIcon name="down" size={12} /></button>
    {#if settings}
        <div class="settings">
            <label>Search coverage<select bind:value={region} onchange={() => { if (text.trim()) run(20); }}>{#each regions as id}<option value={id}>{regionName(id)}</option>{/each}</select></label>
            <label>Trip start date<input type="date" value={context.startDate ?? ''} onchange={e => onDate(e.currentTarget.value)} /></label>
            <button type="button" onclick={onLocation}>{context.here ? 'Update my location' : 'Use my location'}</button>
            <button type="button" onclick={onSample}>Load Black Forest test route</button>
            <p class="note">{HOSTED_SEARCH ? `Maps, routing, and search cover ${regionName(region)}.` : 'Search uses the selected local package. Map tiles have their own coverage.'}</p>
        </div>
    {/if}
</div>
<style>
    .planner-query { padding: 16px 16px 12px; color: var(--ink); }
    form { margin: 0; }
    .query-input { display: flex; align-items: center; gap: 8px; min-height: 40px; padding: 0 10px; border: 1px solid var(--line-strong, var(--line)); border-radius: 6px; background: var(--panel); color: var(--ink-soft); }
    .query-input:focus-within { border-color: var(--ink-soft); outline: 2px solid var(--ink); outline-offset: 2px; }
    .query-input input:focus-visible { outline: none; }
    .query-input input { flex: 1; width: 0; min-width: 0; padding: 10px 0; font: inherit; font-size: 14px; border: 0; outline: none; background: transparent; color: var(--ink); caret-color: var(--ink); }
    input::placeholder { color: var(--ink-soft); opacity: 1; }
    input:focus-visible { outline: none; }
    input::selection { color: var(--panel); background: var(--ink); }
    .edited input:not(:focus) { color: var(--ink-soft); }
    button { font: inherit; color: inherit; cursor: pointer; }
    button:focus-visible, select:focus-visible, input:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .clear { border: 0; background: transparent; width: 28px; height: 32px; padding: 0; display: grid; place-items: center; flex: none; }
    .meaning { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-top: 10px; }
    .meaning-label { font-size: 11px; color: var(--ink-soft); width: 100%; text-transform: capitalize; }
    .note { margin: 8px 0 0; color: var(--ink-soft); font-size: 13px; line-height: 1.45; }
    .data-button { display: flex; align-items: center; gap: 5px; padding: 8px 0 0; min-height: 32px; border: 0; background: transparent; font-size: 11px; color: var(--ink-soft); }
    .settings { display: flex; flex-direction: column; gap: 8px; padding-top: 8px; font-size: 13px; }
    label { display: flex; flex-direction: column; gap: 4px; }
    .settings :is(input, select, button) { border: 1px solid var(--line); border-radius: 6px; color: var(--ink); background: var(--panel); min-height: 36px; padding: 6px; font: inherit; }
    @media (max-width: 700px) { .query-input input { font-size: 16px; } }
</style>
