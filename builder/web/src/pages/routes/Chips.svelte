<!-- The chips of the search of the Routes page. A free chip offers its options; a set chip shows its value. -->
<script lang="ts">
    import { bikes } from '../../lib/library/records';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import { rangeText, type Box, type Chips, type Range } from './filter';
    import PlaceSearch from './PlaceSearch.svelte';
    import RangeSlider from './RangeSlider.svelte';

    type Key = keyof Chips;
    let { chips = $bindable(), view, keys, fixed = [], years = [], grades = [], values }: {
        chips: Chips;
        view?: Box;
        /** The chips to offer, in this order. */
        keys: Key[];
        /** The chips with no "Remove". */
        fixed?: Key[];
        /** The options of the year chip. */
        years?: number[];
        /** The names of the grades of the grade chip, the easiest first. */
        grades?: string[];
        /** The value of each item that the other chips keep, in the units of the rider, for the bars of a slider. */
        values: (key: 'length' | 'climb') => number[];
    } = $props();

    const NAMES: Record<Key, string> = { bike: 'Bike', place: 'Place', length: 'Length', climb: 'Climb', days: 'Days', year: 'Year', grade: 'Grade', shape: 'Shape' };
    const DAYS: { value: Range; label: string }[] = [
        { value: [1, 2], label: 'One day' }, { value: [2, 5], label: '2 to 4 days' }, { value: [5, Infinity], label: '5 days or more' },
    ];
    const SHAPES: { value: Chips['shape']; label: string }[] = [{ value: 'loop', label: 'Loop' }, { value: 'one-way', label: 'One way' }];

    let open = $state<Key | null>(null);

    /** A value in the units of the rider, with its unit and at most one decimal: "40 km", "12.3 km", "1,500 m". */
    const show = (key: 'length' | 'climb') => (value: number) =>
        `${value.toLocaleString('en', { maximumFractionDigits: 1 })}\u00a0${key === 'length' ? settings.units : heightUnit()}`;
    const rangeLabel = (key: 'length' | 'climb', range: Range) => rangeText(range, show(key));

    function options(key: 'bike' | 'days' | 'year' | 'grade' | 'shape'): { value: Chips[Key]; label: string }[] {
        if (key === 'bike') return bikes.map(bike => ({ value: bike, label: ridingProfiles[bike].label }));
        if (key === 'days') return DAYS;
        if (key === 'shape') return SHAPES;
        if (key === 'grade') return grades.map((name, grade) => ({ value: grade, label: `Up to ${name}` }));
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
                    <PlaceSearch {view} label="Place" placeholder="Town, region or lake" autofocus
                        onchoose={place => choose('place', { name: place.name, kind: place.kind, box: place.box })} />
                {:else if key === 'length' || key === 'climb'}
                    <!-- Only the open slider reads its values, and it takes its scale as it opens. -->
                    {#if open === key}<RangeSlider values={values(key)} range={chips[key]} onchange={range => put(key, range)} label={NAMES[key]} format={show(key)} />{/if}
                {:else}
                    {#each options(key) as option (option.label)}
                        <button type="button" aria-pressed={set && label(key) === option.label} onclick={() => choose(key, option.value)}>{option.label}</button>
                    {/each}
                {/if}
                {#if set && !fixed.includes(key)}<button type="button" class="remove" onclick={() => choose(key, undefined)}>Remove</button>{/if}
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
    .menu .remove {
        margin-top: 4px;
        border-top: 1px solid var(--line);
        border-radius: 0 0 5px 5px;
        color: var(--ink-soft);
    }
    /* A finger needs a higher area than the text of a chip. */
    @media (pointer: coarse) {
        .chips {
            row-gap: 14px;
        }
        summary {
            position: relative;
        }
        summary::after {
            position: absolute;
            inset: -7px 0;
            content: '';
        }
    }
    /* On a phone a menu starts at the left of the row, so that it stays on the screen under a chip at the right. */
    @media (max-width: 760px) {
        .chip {
            position: static;
        }
    }
</style>
