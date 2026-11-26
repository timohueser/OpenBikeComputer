<!-- The planner search: the search bar with "Find", "Where" and "When". At Enter the query model reads the sentence on the
     server, and the request of its answer fills the slots; a slot that the answer leaves empty keeps its value. A
     sentence of only search areas and opening times, a suggestion and each change in a panel search with the request of
     the slots and no model. The page reads the slots for the stretch of "Where", and sets it with `setStretch`. -->
<script lang="ts">
    import { onDestroy, untrack } from 'svelte';
    import PlannerIcon from '../../lib/ui/Icon.svelte';
    import SearchBar, { type Slot } from '../../lib/ui/search-bar/SearchBar.svelte';
    import type { FieldProps, Place } from '../../lib/ui/search-bar/Field.svelte';
    import { unfold } from '../../lib/ui/search-bar/motion';
    import type { Value } from '../../lib/ui/search-bar/read';
    import { areasAndTimes, oftenNext, PLANNER_WORDS, saysNearest, type Mark } from '../../lib/planner/search/marks';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import { ACTS, fill, NEAREST, requestOf, resetSlot, sameValue, slotsOf, slotWords, stretchWhere, whereWords, withAct, type SlotId, type Slots, type Stretch } from '../../lib/planner/search/slots';
    import { searchPlaces, type QueryRequest, type SearchAnswer, type SearchContext, type SearchState, type Where } from '../../lib/planner/search/types';
    import FindPanel from './FindPanel.svelte';
    import WherePanel from './WherePanel.svelte';
    import WhenPanel from './WhenPanel.svelte';

    let { searchState = $bindable({ loading: false, error: '', answer: null }), slots = $bindable({}), context, selection, searching = false, revision = 0, viewRevision = 0, onSearch, onClear, onLocation }: {
        searchState?: SearchState; slots?: Slots; context: SearchContext;
        /** The point that the map selects, which "Where" can search near. */
        selection?: Where;
        /** The page shows the answer of the search: the slots stay. */
        searching?: boolean;
        revision?: number; viewRevision?: number; onSearch: () => void; onClear: () => void; onLocation: () => Promise<'denied' | 'unavailable' | undefined>;
    } = $props();

    const VIEW: Where = { scope: 'view' };
    /** One of a place, one of a fact of the route, and one of an action: the label "Find" does not say that it can be one. */
    const EXAMPLES = ['drinking water in the middle of day 1', 'climbs over 300 m on day 2', 'route to the next supermarket'];
    const ICONS: Record<SlotId, string> = { find: 'search', where: 'route', when: 'clock' };

    const request = $derived(requestOf(slots, context.plan));
    /** The request of a change of the plan that the list shows: it fills no slot, so a new search asks for it again. */
    let planned = $state.raw<QueryRequest>();
    /** The text of the field. */
    let typed = $state('');
    /** The last sentence that the query model read, and whether a slot changed since. */
    let sentence = '', edited = false;
    /** The kinds that one thing to find left out of the last answer. */
    let left = $state<string[]>([]);
    /** The search before the cross, for "Undo", and the last searches, the newest first. */
    let searches = $state.raw<Slots[]>([]);
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    let serial = 0, limit = 20;
    let requestContext: SearchContext | undefined;
    let searchedView = $state(0);
    const usesView = $derived(!planned && (request?.type === 'place' ? !request.near : !!request && (request.where ?? VIEW).scope === 'view'));
    const movedView = $derived(usesView && !!searchState.answer && viewRevision !== searchedView);
    const days = $derived(context.plan.days.map(day => day.number));

    /** Asks the server. With `sent`, the server reads no sentence; without it, the query model reads `q`. The answer shows
     * with the `notice` of `told`, the answer of the model that this search repeats with the values of the slots. */
    async function search(q: string, sent: QueryRequest | undefined, nextLimit: number, searchContext = context, told?: SearchAnswer): Promise<SearchAnswer | undefined> {
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
            searchState = { loading: false, error: '', answer: told ? { ...answer, notice: told.notice, canRetry: told.canRetry } : answer };
            return answer;
        } catch (error) {
            if (id !== serial || signal.aborted) return;
            searchState = { loading: false, answer: searchState.answer, error: error instanceof TypeError ? 'Search is unavailable. Check your connection and retry.' : (error as Error).message };
        }
    }
    /** The query model read `q`: the request of its answer fills the slots. A request that changes the plan and that the
     * slots cannot hold fills none. When the slots then give another request, they search again with no model. */
    function took(q: string, answer: SearchAnswer) {
        const found = slotsOf(answer.request);
        [sentence, edited, left, planned] = [q, false, found.left, undefined];
        if (!found.slots.find && !found.slots.where && !found.slots.open) {
            planned = answer.request;
            return;
        }
        // "The next supermarket" is the nearest one ahead on the route: the model reads "next" as no search area.
        const kind = found.slots.find && 'kind' in found.slots.find;
        if (kind && !found.slots.where && context.plan.km.length > 1 && saysNearest(q)) found.slots.where = NEAREST;
        slots = fill(slots, found.slots);
        const { ignored: _, ...asked } = answer.request;
        if (!sameValue(requestOf(slots, context.plan), asked)) run(limit, context, answer);
    }
    /** Searches with the request of the slots and no model. With no thing to find there is no search. */
    function run(nextLimit = 20, searchContext = context, told?: SearchAnswer) {
        if (!request) return stop();
        void search(request.type === 'place' ? request.name! : sentence, request, nextLimit, searchContext, told);
    }
    /** Searches again for what the list shows, a change of the plan or the request of the slots, with no model. */
    function again(nextLimit = limit, searchContext = context) {
        if (planned) void search(sentence, planned, nextLimit, searchContext);
        else run(nextLimit, searchContext);
    }
    function stop() {
        clearTimeout(timer); controller?.abort(); serial++;
        searchState = { loading: false, error: '', answer: null };
    }
    /** Searches again when the changes rest, such as at the end of a drag. */
    function later() {
        clearTimeout(timer); controller?.abort(); serial++;
        searchState = { ...searchState, loading: true, error: '' };
        timer = setTimeout(() => again(), 200);
    }
    /** New values of the slots from a panel, a suggestion or "Undo"; `moving` while a drag changes them. */
    function set(next: Slots, moving = false) {
        [slots, edited, left, planned] = [next, true, [], undefined];
        if (request) return moving ? later() : run();
        stop();
        onClear();
    }
    /** A stretch from km to km of the route is the area of "Where", such as from a drag on the profile of the page. */
    export function setStretch(stretch: Stretch | null) {
        set({ ...slots, where: stretch ? stretchWhere(stretch) : undefined }, !!stretch);
    }
    /** Enter. A sentence of only search areas and opening times sets them from the word list; any other sentence goes to
     * the query model. */
    async function read(text: string): Promise<{ kept: string; values: Value[] } | undefined> {
        const before = slots, marks = areasAndTimes(text);
        if (marks) {
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
    /** The cross closes the search; it has no "Undo". */
    function clear() {
        reset();
        onClear();
    }
    /** Empties the search and the field, such as after a change of the plan. */
    export function reset() {
        stop();
        [slots, sentence, left, planned, typed] = [{}, '', [], undefined, ''];
    }
    export function more() { run(Math.min(100, Math.max(20, limit + 20)), requestContext); }
    /** A failed or a temporary answer: the query model reads the sentence again, unless a slot changed since. */
    export function retry() {
        if (!edited && sentence) void search(sentence, undefined, Math.max(20, limit), requestContext);
        else again(Math.max(20, limit), requestContext);
    }
    $effect(() => {
        // Map movement advances viewRevision instead, so the searched area stays until the rider searches the new view.
        void revision;
        untrack(() => { if (planned || request) later(); });
    });
    onDestroy(() => { clearTimeout(timer); controller?.abort(); });

    const words = $derived(slotWords(slots));
    const named = $derived(request?.type === 'place'), fact = $derived(request?.type === 'stretches');
    const bar = $derived<Slot[]>([
        { id: 'find', icon: ICONS.find, label: slots.act ? ACTS[slots.act].label : 'Find', words: words.find, unset: 'Choose', set: !!slots.find || !!slots.act, wide: true },
        // A named place is searched near a place or in the map view.
        { id: 'where', icon: ICONS.where, label: 'Where', words: words.where, unset: whereWords(VIEW)[0], set: !!slots.where || !!slots.radius, wide: true,
            off: named ? whereWords(request?.near ? { near: [request.near] } : VIEW)[0] : undefined },
        { id: 'when', icon: ICONS.when, label: 'When', words: words.when, unset: 'Any time', set: !!slots.open, off: named || fact ? 'Any time' : undefined },
    ]);
    const summary = (kept: Slots) => Object.values(slotWords(kept)).flat().join(' · ');
    /** The answer has a line under the bar: the kinds left out, a notice, or a change of the plan with "Apply". An
     * empty field then offers nothing, as its list would cover the line. */
    const noted = $derived(!!planned || left.length > 0 || !!searchState.answer?.notice);
    const field = $derived<FieldProps<Mark, Place>>({
        table: PLANNER_WORDS, label: 'Find a place or ask about the route', placeholder: 'Ask, such as “water on day 2”', maxlength: 240, examples: searches.length || noted ? [] : EXAMPLES,
        recent: noted ? [] : searches.filter(kept => !sameValue(kept, slots)).map(kept => ({ text: summary(kept), run: () => set(kept) })),
        often: () => noted ? [] : oftenNext(slots, days.length), changes: mark => !sameValue(mark.set(slots), slots),
        onvalues: marks => set(marks.reduce((next, mark) => mark.set(next), slots)), read,
        onsearch: () => { if (slots.find) searches = [slots, ...searches.filter(kept => !sameValue(kept, slots))].slice(0, 3); },
    });
    let open = $state<string>();

    /** The request of "Find" and "When" along the whole route while the panel of "Where" is open, as text, so that a
     * drag, which changes only the area, asks nothing. */
    const wholeRoute = $derived(open === 'where' && context.plan.km.length > 1 && slots.find && !('name' in slots.find)
        ? JSON.stringify(requestOf({ ...slots, where: { scope: 'route' }, radius: undefined }, context.plan)) : '');
    /** Its answer: the profile of "Where" shows its places or stretches. */
    let alongRoute = $state.raw<SearchAnswer>();
    $effect(() => {
        alongRoute = undefined;
        if (!wholeRoute) return;
        const asked = new AbortController();
        searchPlaces('', untrack(() => context), 100, asked.signal, JSON.parse(wholeRoute)).then(answer => alongRoute = answer, () => {});
        return () => asked.abort();
    });
</script>

<div class="planner-query">
    <SearchBar label="Search the route" slots={bar} {field} folds={!searching} bind:open bind:text={typed} empty={!slots.find && !slots.act && !slots.where && !slots.radius && !slots.open && !planned}
        onreset={id => set(resetSlot(slots, id as SlotId))} onclear={clear}>
        {#snippet panel(id)}
            {#if id === 'find'}
                <FindPanel find={slots.find} act={slots.act} onchange={find => set({ ...slots, find })} onact={act => set(withAct(slots, act))} />
            {:else if id === 'where'}
                <WherePanel where={slots.where ?? VIEW} radius={slots.radius} plan={context.plan} found={alongRoute} {selection} problem={searchState.error} onchange={(change, moving) => set({ ...slots, ...change }, moving)} />
            {:else}
                <WhenPanel open={slots.open} onchange={open => set({ ...slots, open })} />
            {/if}
        {/snippet}
    </SearchBar>
    {#if movedView}<button type="button" class="search-view" in:unfold disabled={searchState.loading} onclick={() => run(limit)}><PlannerIcon name="search" size={14} />Search this map view</button>{/if}
    {#if left.length}<p class="note" role="status" in:unfold>One thing at a time: {left.map(kindLabel).join(', ')} {left.length > 1 ? 'were' : 'was'} left out.</p>{/if}
</div>

<style>
    .planner-query { padding: 16px 16px 12px; color: var(--ink); }
    .search-view { display: flex; align-items: center; justify-content: center; gap: 6px; width: 100%; min-height: 36px; margin-top: 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--parchment-2); color: var(--ink); font: inherit; font-size: 13px; cursor: pointer; }
    .search-view:disabled { opacity: .6; cursor: wait; }
    .search-view:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .note { margin: 8px 0 0; color: var(--ink-soft); font-size: 13px; line-height: 1.45; }
</style>
