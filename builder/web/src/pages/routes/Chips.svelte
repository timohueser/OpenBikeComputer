<!-- The chips of the search of the Routes page. A free chip offers its options; a set chip shows its value. -->
<script lang="ts">
    import { untrack } from 'svelte';
    import { bikes } from '../../lib/library/records';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { distanceIn, formatDistance, formatHeight, heightIn } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import { findPlaces, rangeText, type Box, type Chips, type FoundPlace, type Range } from './filter';
    import RangeSlider from './RangeSlider.svelte';

    type Key = keyof Chips;
    let { chips = $bindable(), view, keys, years = [], values }: {
        chips: Chips;
        view?: Box;
        /** The chips to offer, in this order. */
        keys: Key[];
        /** The options of the year chip. */
        years?: number[];
        /** The value of each item that the other chips keep, in the units of the rider, for the bars of a slider. */
        values: (key: 'length' | 'climb') => number[];
    } = $props();

    const NAMES: Record<Key, string> = { bike: 'Bike', place: 'Place', length: 'Length', climb: 'Climb', days: 'Days', year: 'Year' };
    const DAYS: { value: Range; label: string }[] = [
        { value: [1, 2], label: 'One day' }, { value: [2, 5], label: '2 to 4 days' }, { value: [5, Infinity], label: '5 days or more' },
    ];
    const WAIT_MS = 300;

    let open = $state<Key | null>(null);
    let query = $state(''), found = $state.raw<FoundPlace[] | null>(null), failure = $state('');
    let timer: ReturnType<typeof setTimeout> | undefined, controller: AbortController | undefined;

    /** A value in the units of the rider, with its unit. A length on a step of the slider is a whole number and shows no
     * decimal; a stored length off the step keeps the decimal of `formatDistance`. */
    const show = (key: 'length' | 'climb') => (value: number) =>
        key === 'climb' ? formatHeight(value / heightIn(1))
            : Number.isInteger(value) ? `${value.toLocaleString('en')}\u00a0${settings.units}` : formatDistance(value / distanceIn(1));
    const rangeLabel = (key: 'length' | 'climb', range: Range) => rangeText(range, show(key));

    function options(key: 'bike' | 'days' | 'year'): { value: Chips[Key]; label: string }[] {
        if (key === 'bike') return bikes.map(bike => ({ value: bike, label: ridingProfiles[bike].label }));
        if (key === 'days') return DAYS;
        return years.map(year => ({ value: year, label: String(year) }));
    }

    function label(key: Key): string {
        const value = chips[key];
        if (key === 'place') return chips.place!.name;
        if (key === 'length' || key === 'climb') return rangeLabel(key, value as Range);
        return options(key).find(option => JSON.stringify(option.value) === JSON.stringify(value))?.label ?? NAMES[key];
    }

    // The set chips come first, each group in the order of `keys`. A chip does not move while a menu is open.
    let ordered = $state<Key[]>([]);
    // Another view has other chips and other values, so its menus start closed.
    const offered = $derived(keys.join());
    $effect.pre(() => {
        void offered;
        untrack(() => open = null);
    });
    $effect.pre(() => {
        if (open === null) ordered = [...keys.filter(key => chips[key] !== undefined), ...keys.filter(key => chips[key] === undefined)];
    });

    function put(key: Key, value: Chips[Key] | undefined) {
        const next: Chips = { ...chips };
        if (value === undefined) delete next[key];
        else Object.assign(next, { [key]: value });
        chips = next;
    }

    function choose(key: Key, value: Chips[Key] | undefined) {
        put(key, value);
        open = null;
    }

    function search() {
        clearTimeout(timer);
        controller?.abort();
        const text = query.trim();
        [found, failure] = [null, ''];
        if (!text) return;
        timer = setTimeout(() => {
            const current = controller = new AbortController();
            findPlaces(text, view ?? [-180, -85, 180, 85], current.signal).then(
                places => { if (!current.signal.aborted) found = places; },
                reason => { if (!current.signal.aborted) failure = reason instanceof Error ? reason.message : String(reason); });
        }, WAIT_MS);
    }
</script>

<svelte:window onclick={event => { if (open && !(event.target as Element).closest?.('.chip.open')) open = null; }} />

<div class="chips">
    {#each ordered as key (key)}
        {@const set = chips[key] !== undefined}
        <details class="chip" class:set class:open={open === key} open={open === key}
            ontoggle={event => { if (event.currentTarget.open) open = key; else if (open === key) open = null; }}>
            <summary>{set ? `${label(key)} ▾` : `+ ${NAMES[key]}`}</summary>
            <div class="menu">
                {#if key === 'place'}
                    <!-- svelte-ignore a11y_autofocus -->
                    <input type="search" aria-label="Place" placeholder="Town, region or lake" autofocus bind:value={query} oninput={search} />
                    {#if failure}
                        <p class="note">{failure}</p>
                    {:else if found?.length === 0}
                        <p class="note">No place found.</p>
                    {/if}
                    {#each found ?? [] as place (place.box.join() + place.name)}
                        <button type="button" onclick={() => choose('place', { name: place.name, kind: place.kind, box: place.box })}>
                            {place.name}<small>{[place.kind.replaceAll('_', ' '), place.region].filter(Boolean).join(' · ')}</small>
                        </button>
                    {/each}
                {:else if key === 'length' || key === 'climb'}
                    <!-- Only the open slider reads its values, and it takes its scale as it opens. -->
                    {#if open === key}<RangeSlider values={values(key)} range={chips[key]} onchange={range => put(key, range)} label={NAMES[key]} format={show(key)} />{/if}
                {:else}
                    {#each options(key) as option (option.label)}
                        <button type="button" aria-pressed={set && label(key) === option.label} onclick={() => choose(key, option.value)}>{option.label}</button>
                    {/each}
                {/if}
                {#if set}<button type="button" class="remove" onclick={() => choose(key, undefined)}>Remove</button>{/if}
            </div>
        </details>
    {/each}
</div>

<style>
    .chips {
        position: relative;
        display: flex;
        flex-wrap: wrap;
        gap: 4px 6px;
        margin: -2px -6px;
    }
    .chip {
        position: relative;
    }
    /* A free chip is plain text, so the row is one quiet line; only a set chip is a box. */
    summary {
        padding: 3px 6px;
        border-radius: 6px;
        color: var(--ink-soft);
        font-size: 13px;
        list-style: none;
        cursor: pointer;
        white-space: nowrap;
    }
    summary::-webkit-details-marker {
        display: none;
    }
    summary:hover, .open summary {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .set summary {
        margin: 0 2px 0 6px;
        padding: 3px 9px;
        background: var(--parchment-3);
        color: var(--ink);
    }
    .menu {
        position: absolute;
        z-index: 5;
        top: calc(100% + 4px);
        left: 0;
        display: flex;
        flex-direction: column;
        min-width: 180px;
        padding: 4px;
        border: 1px solid var(--line);
        border-radius: 8px;
        background: var(--panel);
        box-shadow: 0 8px 24px rgb(28 27 20 / 0.18);
    }
    .menu button {
        display: flex;
        flex-direction: column;
        padding: 6px 10px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink);
        text-align: left;
        white-space: nowrap;
    }
    .menu button:hover, .menu [aria-pressed="true"] {
        background: var(--parchment-2);
    }
    .menu small {
        color: var(--ink-soft);
    }
    .menu .remove {
        margin-top: 4px;
        border-top: 1px solid var(--line);
        border-radius: 0 0 5px 5px;
        color: var(--ink-soft);
    }
    input {
        min-width: 220px;
        margin: 2px 2px 4px;
        padding: 6px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
    }
    .note {
        margin: 4px 8px 6px;
        color: var(--ink-soft);
    }
    /* On a phone a menu starts at the left of the row, so that it stays on the screen under a chip at the right. */
    @media (max-width: 760px) {
        .chip {
            position: static;
        }
    }
</style>
