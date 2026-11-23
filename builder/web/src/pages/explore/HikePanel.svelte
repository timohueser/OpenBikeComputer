<!-- The panel of "Hike": the kind, the shape, the length, the climb and the path difficulty. Each row shows its value; a
     click shows its control under it and closes the row that was open. Only hiking can be chosen. On the long trails tab
     only the kind and the hardest grade apply. -->
<script lang="ts">
    import { ridingProfiles, type BikeType } from '../../lib/planner/riding-profiles';
    import { distanceIn, formatHeight, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { unfold } from '../../lib/ui/search-bar/motion';
    import { rangeText, scale, type Range } from '../routes/filter';
    import RangeSlider from '../routes/RangeSlider.svelte';
    import GradeRange from './GradeRange.svelte';
    import type { Query } from './search';
    import Shapes from './Shapes.svelte';
    import { gradeText, shapesSet } from './slots';
    import { CLIMB_TOP_M, GRADES, LENGTH_TOP_M, SHAPE_WORDS, shortDistance } from './words';

    let { query, trails, lifts, ungraded, lengths, climbs, onchange }: {
        /** The query that the page searches, with the values that are off at their defaults. */
        query: Query;
        /** The long trails tab: the shape, the length and the climb do not apply. */
        trails: boolean;
        /** Lifts can apply: the release has facts files. */
        lifts: boolean;
        /** The rows with no grade that "Include routes with an unknown grade" keeps. */
        ungraded: number;
        /** The length and the climb of each row that the other values keep, in the units of the rider. */
        lengths: number[];
        climbs: number[];
        onchange: (change: Partial<Query>) => void;
    } = $props();

    type Row = 'kind' | 'shape' | 'length' | 'climb' | 'grade';
    const KINDS: BikeType[] = ['hiking', 'road', 'gravel', 'touring', 'mtb'];
    let open = $state<Row>();

    /** A value in the units of the rider: "12.5 km", "1,500 m". */
    const show = (unit: () => string) => (value: number) => `${value.toLocaleString('en', { maximumFractionDigits: 1 })} ${unit()}`;
    /** A range of the query in metres, in the units of the rider, and back. */
    const shown = (range: Range | undefined, units: (metres: number) => number): Range | undefined => range && [units(range[0]), units(range[1])];
    const metres = (range: Range | undefined, units: (metres: number) => number): Range | undefined => range && [Math.round(range[0] / units(1)), Math.round(range[1] / units(1))];

    const grade = $derived(gradeText(query));
    const rows = $derived<{ id: Row; icon: string; label: string; value?: string; off?: boolean }[]>([
        { id: 'kind', icon: 'hiking', label: 'Kind', value: ridingProfiles.hiking.label },
        { id: 'shape', icon: 'loop', label: 'Shape', value: shapesSet(query).map(shape => SHAPE_WORDS[shape].tag).join(', ') || undefined, off: trails },
        { id: 'length', icon: 'route', label: 'Length', value: query.length && rangeText(query.length, shortDistance), off: trails },
        { id: 'climb', icon: 'ascent', label: 'Climb', value: query.climb && rangeText(query.climb, formatHeight), off: trails },
        { id: 'grade', icon: 'mountain', label: 'Path difficulty', value: grade && query.grade[0] === 0 ? `${grade} · ${GRADES[query.grade[1]][0]}` : grade },
    ]);
</script>

<div class="rows">
    {#each rows as row (row.id)}
        <div class="row" class:open={open === row.id}>
            <button type="button" class="head" disabled={row.off} aria-expanded={open === row.id} title={row.off ? 'Not for long trails' : undefined}
                onclick={() => open = open === row.id ? undefined : row.id}>
                <Icon name={row.icon} size={16} /><span>{row.label}</span><em class:set={!!row.value}>{row.value ?? 'Any'}</em><Icon name="down" size={14} />
            </button>
            {#if open === row.id && !row.off}
                <div class="body" transition:unfold>
                    {#if row.id === 'kind'}
                        <div class="kinds" role="radiogroup" aria-label="Kind of route">
                            {#each KINDS as kind (kind)}
                                <button type="button" role="radio" aria-checked={kind === 'hiking'} disabled={kind !== 'hiking'}>
                                    <Icon name={ridingProfiles[kind].icon} size={16} />{ridingProfiles[kind].label}
                                </button>
                            {/each}
                        </div>
                        <p class="hint">Bike routes come later.</p>
                    {:else if row.id === 'shape'}
                        <Shapes {query} {lifts} {onchange} />
                    {:else if row.id === 'length'}
                        <!-- The sliders keep one scale while the loaded hikes change. -->
                        <RangeSlider values={lengths} fixed={scale([distanceIn(LENGTH_TOP_M)]).step} range={shown(query.length, distanceIn)} label="Length" reading={false}
                            format={show(() => settings.units)} onchange={range => onchange({ length: metres(range, distanceIn) })} />
                    {:else if row.id === 'climb'}
                        <RangeSlider values={climbs} fixed={scale([heightIn(CLIMB_TOP_M)]).step} range={shown(query.climb, heightIn)} label="Climb" reading={false}
                            format={show(heightUnit)} onchange={range => onchange({ climb: metres(range, heightIn) })} />
                    {:else}
                        <GradeRange {query} {ungraded} upper={trails} {onchange} />
                    {/if}
                </div>
            {/if}
        </div>
    {/each}
    {#if trails}<p class="hint off">Not for long trails: Shape, Length and Climb.</p>{/if}
</div>

<style>
    .rows {
        width: 400px;
        max-width: 100%;
        padding: 4px;
    }
    .row + .row {
        border-top: 1px solid var(--line);
    }
    .head {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        min-height: 46px;
        padding: 8px 10px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
        transition: background 120ms;
    }
    .head:hover:not(:disabled) {
        background: var(--parchment);
    }
    .head:active:not(:disabled) {
        background: var(--parchment-2);
    }
    .head:disabled {
        cursor: default;
        opacity: 0.45;
    }
    .head > :global(svg:first-child) {
        flex: none;
        color: var(--ink-soft);
    }
    .head span {
        flex: 1;
        font-weight: 600;
    }
    em {
        color: var(--ink-faint);
        font-style: normal;
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
    em.set {
        color: var(--ink);
    }
    .head > :global(svg:last-child) {
        flex: none;
        color: var(--ink-faint);
        transition: transform 180ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .open .head > :global(svg:last-child) {
        transform: rotate(180deg);
    }
    .body {
        display: flex;
        flex-direction: column;
        gap: 10px;
        padding: 2px 10px 14px;
    }
    /* The slider of a row is as wide as the row. */
    .body :global(.slider) {
        width: auto;
    }
    @media (prefers-reduced-motion: reduce) {
        .head > :global(svg:last-child) {
            transition: none;
        }
    }
    .kinds {
        display: flex;
        flex-wrap: wrap;
        gap: 2px;
        padding: 2px;
        border-radius: 6px;
        background: var(--parchment-2);
    }
    .kinds button {
        display: inline-flex;
        flex: 1 1 auto;
        align-items: center;
        justify-content: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 9px;
        border: 0;
        border-radius: 5px;
        background: transparent;
        color: var(--ink-soft);
        font: 600 12.5px var(--sans);
        white-space: nowrap;
    }
    .kinds button:disabled {
        cursor: default;
        opacity: 0.45;
    }
    .kinds [aria-checked="true"] {
        background: var(--panel);
        color: var(--ink);
        box-shadow: inset 0 0 0 1px var(--line-strong);
    }
    .hint {
        margin: 0;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .off {
        padding: 10px 10px 8px;
        border-top: 1px solid var(--line);
    }
</style>
