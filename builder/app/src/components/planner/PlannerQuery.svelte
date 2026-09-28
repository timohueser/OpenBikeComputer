<script lang="ts">
    import PlannerIcon from './PlannerIcon.svelte';
    import { parsePlannerQuery, type PlannerQueryValue } from '../../lib/planner/query';

    let { text = $bindable(''), days, onSearch, onClear }: {
        text?: string;
        days: number;
        onSearch: (query: PlannerQueryValue) => void;
        onClear?: () => void;
    } = $props();

    let edits = $state<Partial<PlannerQueryValue>>({});
    let editedText = $state('');
    let picker = $state<'category' | 'day' | 'within' | null>(null);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const categories: { value: PlannerQueryValue['category']; label: string; icon: string }[] = [
        { value: 'all', label: 'All places', icon: 'pin' },
        { value: 'hotel', label: 'Hotels', icon: 'hotel' },
        { value: 'camp', label: 'Campsites', icon: 'camp' },
        { value: 'water', label: 'Drinking water', icon: 'water' },
    ];
    const parsed = $derived(parsePlannerQuery(text));
    const value = $derived<PlannerQueryValue>({ ...parsed.value, ...(editedText === text ? edits : {}) });
    const edited = $derived(editedText === text && Object.keys(edits).length > 0);
    const selectedCategory = $derived(categories.find(category => category.value === value.category)!);
    const invalidDay = $derived(value.day !== null && (value.day < 1 || value.day > days));
    const understood = $derived(text.trim().length > 0 && !parsed.unsupported && !invalidDay);

    function submit() {
        clearTimeout(timer);
        if (understood) onSearch(value);
    }
    function input() {
        edits = {};
        picker = null;
        clearTimeout(timer);
        if (!text.trim()) onClear?.();
        else timer = setTimeout(submit, 250);
    }
    function edit(change: Partial<PlannerQueryValue>) {
        clearTimeout(timer);
        const previous = editedText === text ? edits : {};
        editedText = text;
        edits = { ...previous, ...change };
        picker = null;
        const next = { ...value, ...change };
        if (!parsed.unsupported && (next.day === null || (next.day >= 1 && next.day <= days))) onSearch(next);
    }
    function clear() {
        clearTimeout(timer);
        text = '';
        edits = {};
        picker = null;
        onClear?.();
    }
    $effect(() => () => clearTimeout(timer));
</script>

<div class="planner-query">
    <form onsubmit={(event) => { event.preventDefault(); submit(); }}>
        <div class="query-input" class:edited>
            <PlannerIcon name="search" size={17} />
            <input aria-label="Search places along your route" bind:value={text} oninput={(event) => { text = event.currentTarget.value; input(); }}
                placeholder="Find a place, or ask along your route…" maxlength="120"
                onkeydown={(event) => { if (event.key === 'Escape') picker = null; }} />
            {#if text}<button type="button" class="clear" aria-label="Clear search" onclick={clear}><PlannerIcon name="close" size={16} /></button>{/if}
        </div>
    </form>
    {#if text.trim()}
        {#if parsed.unsupported}
            <p class="query-note" role="status">This prototype understands hotels, campsites or water, a day, and “within … km”. It does not understand “{parsed.unsupported}”.</p>
        {:else}
            <div class="meaning">
                <span class="meaning-label">{edited ? 'Edited to' : 'Understood as'}</span>
                <button type="button" class="chip" class:active={picker === 'category'} aria-expanded={picker === 'category'} onclick={() => picker = picker === 'category' ? null : 'category'}>
                    <PlannerIcon name={selectedCategory.icon} size={14} />{selectedCategory.label}<PlannerIcon name="down" size={12} />
                </button>
                <button type="button" class="chip" class:active={picker === 'day'} aria-expanded={picker === 'day'} onclick={() => picker = picker === 'day' ? null : 'day'}>
                    {value.day === null ? 'Along the route' : `End of Day ${value.day}`}<PlannerIcon name="down" size={12} />
                </button>
                {#if value.within !== null || (editedText === text && 'within' in edits)}
                    <button type="button" class="chip" class:active={picker === 'within'} aria-expanded={picker === 'within'} onclick={() => picker = picker === 'within' ? null : 'within'}>
                        {value.within === null ? 'Any distance' : `Within ${value.within} km`}<PlannerIcon name="down" size={12} />
                    </button>
                {/if}
            </div>
            {#if invalidDay}<p class="query-note" role="status">This trip has {days} {days === 1 ? 'day' : 'days'}. Edit the day chip to choose another day.</p>{/if}
            {#if picker}
                <div class="picker" aria-label={`Edit search ${picker}`}>
                    {#if picker === 'category'}
                        {#each categories as category}
                            <button type="button" class:chosen={value.category === category.value} aria-pressed={value.category === category.value} onclick={() => edit({ category: category.value })}>
                                <PlannerIcon name={category.icon} size={15} />{category.label}
                            </button>
                        {/each}
                    {:else if picker === 'day'}
                        <button type="button" class:chosen={value.day === null} aria-pressed={value.day === null} onclick={() => edit({ day: null })}>Along the route</button>
                        {#each Array.from({ length: days }, (_, i) => i + 1) as day}
                            <button type="button" class:chosen={value.day === day} aria-pressed={value.day === day} onclick={() => edit({ day })}>Day {day}</button>
                        {/each}
                    {:else}
                        {#each [1, 2, 5, 10, 20] as within}
                            <button type="button" class:chosen={value.within === within} aria-pressed={value.within === within} onclick={() => edit({ within })}>{within} km</button>
                        {/each}
                        <button type="button" onclick={() => edit({ within: null })}>No distance limit</button>
                    {/if}
                </div>
            {/if}
        {/if}
    {/if}
</div>

<style>
    .planner-query { padding: 12px 14px; border-bottom: 1px solid var(--line); color: var(--ink); }
    form { margin: 0; }
    .query-input { display: flex; align-items: center; gap: 8px; min-height: 40px; padding: 0 10px; border: 1px solid var(--line-strong, var(--line)); border-radius: 7px; background: var(--panel); color: var(--ink-soft); }
    .query-input:focus-within { outline: 2px solid var(--ink); outline-offset: 2px; }
    input { flex: 1; width: 0; min-width: 0; padding: 10px 0; font: inherit; font-size: 13px; border: 0; outline: none; background: transparent; color: var(--ink); caret-color: var(--ink); }
    input::placeholder { color: var(--ink-soft); opacity: 1; }
    input::selection { color: var(--panel); background: var(--ink); }
    .edited input:not(:focus) { color: var(--ink-soft); }
    button { font: inherit; color: inherit; cursor: pointer; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .clear { border: 0; background: transparent; width: 28px; height: 32px; padding: 0; display: grid; place-items: center; flex: none; }
    .clear:hover { color: var(--ink); }
    .meaning { display: flex; flex-wrap: wrap; align-items: center; gap: 5px; margin-top: 9px; }
    .meaning-label { font-size: 11px; color: var(--ink-soft); margin-right: 2px; }
    .chip { display: inline-flex; align-items: center; gap: 5px; min-height: 29px; padding: 4px 7px; border: 1px solid var(--line); border-radius: 5px; background: var(--panel); font-size: 11px; white-space: nowrap; }
    .chip:hover, .chip.active { border-color: var(--ink-soft); background: var(--parchment-2, var(--panel)); }
    .picker { display: flex; flex-wrap: wrap; gap: 5px; margin-top: 8px; padding-top: 8px; border-top: 1px solid var(--line); }
    .picker button { display: inline-flex; align-items: center; justify-content: center; gap: 6px; min-height: 34px; padding: 6px 9px; border: 1px solid var(--line); border-radius: 5px; background: var(--panel); font-size: 12px; }
    .picker button:hover { border-color: var(--ink-soft); }
    .picker button.chosen { color: var(--panel); background: var(--ink); border-color: var(--ink); }
    .query-note { margin: 9px 0 0; color: var(--ink-soft); font-size: 12px; line-height: 1.45; }
</style>
