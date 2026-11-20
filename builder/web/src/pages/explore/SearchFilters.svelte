<!-- The filter column of the day-hike search: the day, getting there, length and climb, the shapes, the grade and the
     wishes, all visible at once. Each change gives a new query. -->
<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { FactsPlace } from '../../lib/planner/signed-routes';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { scale as fit, type Range } from '../routes/filter';
    import RangeSlider from '../routes/RangeSlider.svelte';
    import GradeRange from './GradeRange.svelte';
    import Journey from './Journey.svelte';
    import type { Query, Wish } from './search';
    import Shapes from './Shapes.svelte';
    import Wishes from './Wishes.svelte';
    import { CLIMB_TOP_M, LENGTH_TOP_M } from './words';

    let { query, today, cap, counts, ungraded, lengths, climbs, place, fresh, top, onchange, onreset }: {
        query: Query;
        today: string;
        cap: number;
        counts: Map<Wish, number | null>;
        ungraded: number;
        /** The length and the climb of each row that the other filters keep, in the units of the rider. */
        lengths: number[];
        climbs: number[];
        place: (id: string) => FactsPlace | undefined;
        /** No filter differs from its default. */
        fresh: boolean;
        /** Beside the title. */
        top?: Snippet;
        onchange: (change: Partial<Query>) => void;
        onreset: () => void;
    } = $props();

    /** A value in the units of the rider: "12.5 km", "1,500 m". */
    const show = (unit: () => string) => (value: number) => `${value.toLocaleString('en', { maximumFractionDigits: 1 })} ${unit()}`;
    /** A range of the query in metres, in the units of the rider, and back. */
    const shown = (range: Range | undefined, units: (metres: number) => number): Range | undefined => range && [units(range[0]), units(range[1])];
    const metres = (range: Range | undefined, units: (metres: number) => number): Range | undefined => range && [Math.round(range[0] / units(1)), Math.round(range[1] / units(1))];
</script>

<div class="filters">
    <div class="top"><h2>Filters</h2>{@render top?.()}<button type="button" class="reset" disabled={fresh} onclick={onreset}>Reset</button></div>
    <div class="scroll">
        <section>
            <div class="head">
                <h3>Day of the hike</h3>
                <label class="date"><Icon name="calendar" size={16} />
                    <input type="date" min={today} value={query.date} aria-label="Day of the hike"
                        onchange={event => { const date = event.currentTarget.value; if (date >= today) onchange({ date }); }} />
                </label>
            </div>
        </section>
        <Journey {query} {cap} {place} {onchange} />
        <!-- The sliders keep one scale while the loaded hikes change. -->
        <section>
            <div class="head"><h3>Length</h3></div>
            <RangeSlider values={lengths} fixed={fit([distanceIn(LENGTH_TOP_M)]).step} range={shown(query.length, distanceIn)} label="Length"
                format={show(() => settings.units)} onchange={range => onchange({ length: metres(range, distanceIn) })} />
        </section>
        <section>
            <div class="head"><h3>Climb</h3></div>
            <RangeSlider values={climbs} fixed={fit([heightIn(CLIMB_TOP_M)]).step} range={shown(query.climb, heightIn)} label="Climb"
                format={show(heightUnit)} onchange={range => onchange({ climb: metres(range, heightIn) })} />
        </section>
        <Shapes {query} {onchange} />
        <GradeRange {query} {ungraded} {onchange} />
        <Wishes {query} {counts} {place} {onchange} />
    </div>
</div>

<style>
    .filters {
        display: flex;
        flex: 1;
        flex-direction: column;
        min-height: 0;
        margin: 0 -16px -16px;
    }
    .top {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 2px 10px 6px 16px;
    }
    h2 {
        margin: 0;
        font-size: 16px;
        font-weight: 700;
    }
    .reset {
        margin-left: auto;
        padding: 4px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
    }
    .reset:hover:not(:disabled) {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .scroll {
        display: flex;
        flex: 1;
        flex-direction: column;
        gap: 28px;
        min-height: 0;
        overflow-y: auto;
        padding: 8px 16px 32px;
    }
    /* Each group: a heading with its value on the right, then its control. */
    .scroll :global(section) {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .scroll :global(.head) {
        display: flex;
        align-items: center;
        gap: 4px;
        min-height: 26px;
    }
    .scroll :global(h3) {
        margin: 0;
        font-size: 14px;
        font-weight: 600;
    }
    .scroll :global(.value) {
        margin-left: auto;
        color: var(--ink-soft);
        font-size: 13px;
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
    .scroll :global(.hint) {
        display: flex;
        align-items: flex-start;
        gap: 6px;
        margin: 0;
        color: var(--ink-soft);
        font-size: 13px;
        line-height: 1.4;
    }
    .scroll :global(.hint svg) {
        flex: none;
        margin-top: 1px;
    }
    .date {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        height: 32px;
        margin-left: auto;
        padding: 0 4px 0 10px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
        color: var(--ink-soft);
    }
    .date:focus-within {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
    .date input {
        padding: 4px 2px;
        border: 0;
        background: none;
        color: var(--ink);
        font: 600 13.5px var(--sans);
    }
    .date input:focus-visible {
        outline: none;
    }
</style>
