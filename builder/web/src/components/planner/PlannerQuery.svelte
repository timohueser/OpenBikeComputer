<!-- The planner search: the search bar with "Find", "Where" and "When". At Enter the query model reads the sentence on the
     server, and the request of its answer fills the slots; a slot that the answer leaves empty keeps its value. A
     sentence of only search areas and opening times, a suggestion and each change in a panel search with the request of
     the slots and no model. -->
<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import PlannerIcon from '../../lib/ui/Icon.svelte';
    import SearchBar, { type Slot } from '../../lib/ui/search-bar/SearchBar.svelte';
    import type { FieldProps, Place } from '../../lib/ui/search-bar/Field.svelte';
    import { scan, type Value } from '../../lib/ui/search-bar/read';
    import { oftenNext, PLANNER_WORDS, type Mark } from '../../lib/planner/search/marks';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { fill, requestOf, resetSlot, sameValue, slotsOf, slotWords, whereWords, type SlotId, type Slots } from '../../lib/planner/search/slots';
    import { searchPlaces, type QueryRequest, type SearchAnswer, type SearchContext, type SearchState, type Where } from '../../lib/planner/search/types';
    import FindPanel from './FindPanel.svelte';
    import WherePanel from './WherePanel.svelte';
    import WhenPanel from './WhenPanel.svelte';

    let { searchState = $bindable({ loading: false, error: '', answer: null }), context, selection, revision = 0, viewRevision = 0, onSearch, onClear, onLocation }: {
        searchState?: SearchState; context: SearchContext; selection?: Where;
        revision?: number; viewRevision?: number; onSearch: () => void; onClear: () => void; onLocation: () => Promise<'denied' | 'unavailable' | undefined>;
    } = $props();

    const VIEW: Where = { scope: 'view' };
    const EXAMPLES = ['drinking water in the middle of day 1', 'climbs over 300 m on day 2', 'supermarket open now'];
    const ICONS: Record<SlotId, string> = { find: 'search', where: 'route', when: 'clock' };

    let slots = $state.raw<Slots>({});
    const request = $derived(requestOf(slots));
    /** The last sentence that the query model read, and whether a slot changed since. */
    let sentence = '', edited = false;
    /** The kinds that one thing to find left out of the last answer. */
    let left = $state<string[]>([]);
    /** The slots before the cross, for "Undo", and the last searches, the newest first. */
    let cleared: Slots = {}, searches = $state.raw<Slots[]>([]);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let serial = 0, limit = 20;
    let requestContext: SearchContext | undefined;
    let searchedView = $state(0);
    const usesView = $derived(request?.type === 'place' ? !request.near : !!request && (request.where ?? context.pointing ?? VIEW).scope === 'view');
    const movedView = $derived(usesView && !!searchState.answer && viewRevision !== searchedView);
    const days = $derived(context.plan.days.filter(day => !day.rest).map(day => day.number));

    /** Asks the server. With `sent`, the server reads no sentence; without it, the query model reads `q`. */
    async function search(q: string, sent: QueryRequest | undefined, nextLimit: number, searchContext = context): Promise<SearchAnswer | undefined> {
        clearTimeout(timer); controller?.abort(); const id = ++serial;
        controller = new AbortController(); limit = nextLimit; requestContext = searchContext;
        const signal = controller.signal;
        onSearch(); searchedView = viewRevision;
        searchState = { loading: true, error: '', answer: searchState.answer };
        try {
            const answer = await searchPlaces(q, searchContext, limit, signal, sent ? $state.snapshot(sent) : undefined);
            if (id !== serial) return;
            if (!sent) took(q, answer);
            // The slots search again for what they show.
            if (id !== serial) return answer;
            if (answer.code === 'location') {
                // Setting `here` bumps `revision`, which runs the search again.
                const failure = await onLocation();
                if (id !== serial || !failure) return answer;
                throw new Error(failure === 'denied' ? 'Allow location access to search near you.' : 'Your position is not available.');
            }
            searchState = { loading: false, error: '', answer };
            return answer;
        } catch (error) {
            if (id !== serial || signal.aborted) return;
            searchState = { loading: false, answer: searchState.answer, error: error instanceof TypeError ? 'Search is unavailable. Check your connection and retry.' : (error as Error).message };
        }
    }
    /** The query model read `q`: the request of its answer fills the slots. A request that changes the plan fills none.
     * When the slots then give another request, they search again with no model. */
    function took(q: string, answer: SearchAnswer) {
        const found = slotsOf(answer.request);
        [sentence, edited, left] = [q, false, found.left];
        if (!found.slots.find && !found.slots.where && !found.slots.open) return;
        slots = fill(slots, found.slots);
        const { ignored: _, ...asked } = answer.request;
        if (!sameValue(requestOf(slots), asked)) run(limit);
    }
    /** Searches with the request of the slots and no model. With no thing to find there is no search. */
    function run(nextLimit = 20, searchContext = context) {
        if (!request) return stop();
        void search(request.type === 'place' ? request.name! : sentence, request, nextLimit, searchContext);
    }
    function stop() {
        clearTimeout(timer); controller?.abort(); serial++;
        searchState = { loading: false, error: '', answer: null };
    }
    /** New values of the slots from a panel, a suggestion or "Undo". */
    function set(next: Slots) {
        [slots, edited, left] = [next, true, []];
        if (request) return run();
        stop();
        onClear();
    }
    /** Enter. A sentence of only search areas and opening times sets them from the word list; any other sentence goes to
     * the query model. */
    async function read(text: string): Promise<{ kept: string; values: Value[] } | undefined> {
        const before = slots, parts = scan(text, PLANNER_WORDS), marks = parts.flatMap(part => part.kind === 'value' ? [part.value] : []);
        if (marks.length && parts.every(part => part.kind === 'glue' || (part.kind === 'value' && part.value.slot !== 'find'))) {
            set(marks.reduce((next, mark) => mark.set(next), slots));
            return { kept: '', values: landed(before) };
        }
        const answer = await search(text, undefined, 20);
        return answer && { kept: answer.request.ignored?.join(' ') ?? '', values: landed(before) };
    }
    /** The words of each slot that changed since `before`. */
    function landed(before: Slots): Value[] {
        const was = slotWords(before), now = slotWords(slots);
        return (['find', 'where', 'when'] as const).flatMap(slot => now[slot].length && !sameValue(now[slot], was[slot]) ? [{ slot, label: now[slot].join(' · '), icon: ICONS[slot] }] : []);
    }
    function clear() {
        cleared = slots;
        reset();
        onClear();
    }
    /** Empties the search with no "Undo", such as after a change of the plan. */
    export function reset() {
        stop();
        [slots, sentence, left] = [{}, '', []];
    }
    export function more() { run(Math.min(100, Math.max(20, limit + 20)), requestContext); }
    /** A failed or a temporary answer: the query model reads the sentence again, unless a slot changed since. */
    export function retry() {
        if (!edited && sentence) void search(sentence, undefined, Math.max(20, limit), requestContext);
        else run(Math.max(20, limit), requestContext);
    }
    $effect(() => {
        // Map movement advances viewRevision instead, so the searched area stays until the rider searches the new view.
        void revision;
        untrack(() => { if (request) { clearTimeout(timer); controller?.abort(); serial++; searchState = { ...searchState, loading: true, error: '' }; timer = setTimeout(() => run(limit), 200); } });
    });
    onDestroy(() => { clearTimeout(timer); controller?.abort(); });

    const words = $derived(slotWords(slots));
    const named = $derived(request?.type === 'place'), fact = $derived(request?.type === 'stretches');
    const bar = $derived<Slot[]>([
        { id: 'find', icon: ICONS.find, label: 'Find', words: words.find, unset: 'Choose', set: !!slots.find },
        // A named place is searched near a place or in the map view.
        { id: 'where', icon: ICONS.where, label: 'Where', words: words.where, unset: whereWords(context.pointing ?? VIEW)[0], set: !!slots.where || !!slots.radius,
            off: named ? whereWords(request?.near ? { near: [request.near] } : VIEW)[0] : undefined },
        { id: 'when', icon: ICONS.when, label: 'When', words: words.when, unset: 'Any time', set: !!slots.open, off: named || fact ? 'Any time' : undefined },
    ]);
    const summary = (kept: Slots) => Object.values(slotWords(kept)).flat().join(' · ');
    const field = $derived<FieldProps<Mark, Place>>({
        table: PLANNER_WORDS, label: 'Find a place or ask about the route', placeholder: 'Ask, such as “water on day 2”', examples: searches.length ? [] : EXAMPLES,
        recent: searches.filter(kept => !sameValue(kept, slots)).map(kept => ({ text: summary(kept), run: () => set(kept) })),
        often: () => oftenNext(slots, days.length), changes: mark => !sameValue(mark.set(slots), slots),
        onvalues: marks => set(marks.reduce((next, mark) => mark.set(next), slots)), read,
        onsearch: () => { if (slots.find) searches = [slots, ...searches.filter(kept => !sameValue(kept, slots))].slice(0, 3); },
    });
    let open = $state<string>();
</script>

<div class="planner-query">
    <SearchBar label="Search the route" slots={bar} {field} bind:open empty={!slots.find && !slots.where && !slots.radius && !slots.open}
        onreset={id => set(resetSlot(slots, id as SlotId))} onclear={clear} onundo={() => set(cleared)}>
        {#snippet panel(id)}
            {#if id === 'find'}
                <FindPanel find={slots.find} onchange={find => set({ ...slots, find })} />
            {:else if id === 'where'}
                <WherePanel where={slots.where ?? context.pointing ?? VIEW} radius={slots.radius} {days} {selection} onchange={change => set({ ...slots, ...change })} />
            {:else}
                <WhenPanel open={slots.open} onchange={open => set({ ...slots, open })} />
            {/if}
        {/snippet}
    </SearchBar>
    {#if movedView}<button type="button" class="search-view" disabled={searchState.loading} onclick={() => run(limit)}><PlannerIcon name="search" size={14} />Search this map view</button>{/if}
    {#if left.length}<p class="note" role="status">One thing at a time: {left.map(kindLabel).join(', ')} {left.length > 1 ? 'were' : 'was'} left out.</p>{/if}
</div>

<style>
    .planner-query { padding: 16px 16px 12px; color: var(--ink); }
    .search-view { display: flex; align-items: center; justify-content: center; gap: 6px; width: 100%; min-height: 36px; margin-top: 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--parchment-2); color: var(--ink); font: inherit; font-size: 13px; cursor: pointer; }
    .search-view:disabled { opacity: .6; cursor: wait; }
    .search-view:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .note { margin: 8px 0 0; color: var(--ink-soft); font-size: 13px; line-height: 1.45; }
</style>
