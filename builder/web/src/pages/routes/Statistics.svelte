<!-- The window of the statistics: the totals of each year and of each month of a year, the kinds of bike and the largest
     values, by bike or on foot. -->
<script lang="ts">
    import { tick } from 'svelte';
    import type { RideRecord } from '../../lib/library/records';
    import { byKind, byMonth, byYear, inYear, largest, onFoot, sum, type Measure, type Totals } from '../../lib/library/statistics';
    import type { Trip } from '../../lib/library/trips';
    import { ridingProfiles } from '../../lib/planner/riding-profiles';
    import { ROUTES } from '../../lib/routes';
    import { distanceIn, formatCount, formatDistance, formatHeight, formatHours, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';
    import Segmented, { radioKey } from '../../lib/ui/Segmented.svelte';
    import Window from '../../lib/ui/Window.svelte';
    import { dayText } from './rows';

    /** `rides` and `trips` are of the whole library: no search of the list changes the window. */
    let { rides, trips, onClose, onTrip }: {
        rides: RideRecord[]; trips: Trip[]; onClose: () => void;
        /** Shows the row of a trip in the list. */
        onTrip: (id: string) => void;
    } = $props();

    const MEASURES: Measure[] = ['lengthM', 'climbM', 'movingS', 'count'];
    const MONTHS = Array.from({ length: 12 }, (_, month) => new Date(2000, month).toLocaleDateString('en-GB', { month: 'short' }));

    let foot = $state(false), measure = $state<Measure>('lengthM'), year = $state<number>();
    /** The bar under the pointer, with the focus or after a tap: a narrow bar shows its number only then, one at a time. */
    let active = $state<string>();
    let root: HTMLElement, back = $state<HTMLButtonElement>();

    const both = $derived(rides.some(onFoot) && rides.some(ride => !onFoot(ride)));
    const walks = $derived(foot || !rides.some(ride => !onFoot(ride)));
    const mine = $derived(rides.filter(ride => onFoot(ride) === walks));
    const years = $derived(byYear(mine, new Date().getFullYear()));
    /** A statistic with rides in one year has no level of years. */
    const only = $derived(years.filter(part => part.totals.count).length === 1 ? years.find(part => part.totals.count)!.year : undefined);
    const shownYear = $derived(only ?? year);
    const shown = $derived(shownYear === undefined ? mine : inYear(mine, shownYear));
    const totals = $derived(sum(shown));
    const bars = $derived(shownYear === undefined ? years.map(part => ({ ...part, name: `${part.year}` }))
        : byMonth(mine, shownYear).map((part, month) => ({ ...part, name: MONTHS[month], year: undefined })));
    /** The kinds of the statistic, the one with the most distance first. A kind keeps its place and its colour in each year. */
    const order = $derived([...byKind(mine)].sort(([, a], [, b]) => b.lengthM - a.lengthM).map(([bike]) => bike));
    const kinds = $derived(byKind(shown));
    const best = $derived(largest(mine, trips, shownYear));

    const labels = $derived<Record<Measure, string>>({ lengthM: 'Distance', climbM: 'Climb', movingS: 'Time', count: walks ? 'Hikes' : 'Rides' });
    /** The value of a measure in the units of the rider, for the height of a bar and the number over it. */
    const amount = (key: Measure, of: Totals) =>
        key === 'lengthM' ? distanceIn(of.lengthM) : key === 'climbM' ? heightIn(of.climbM) : key === 'movingS' ? of.movingS / 3600 : of.count;
    const text = (key: Measure, of: Totals) => key === 'lengthM' ? formatDistance(of.lengthM) : key === 'climbM' ? formatHeight(of.climbM)
        : key === 'movingS' ? formatHours(of.movingS) : formatCount(of.count);
    const top = $derived(Math.max(...bars.map(bar => amount(measure, bar.totals)), 0));

    /** The bars that take the focus leave with the view, so the focus goes to the way back, and back to the year. */
    async function show(next: number | undefined) {
        const from = year;
        [year, active] = [next, undefined];
        await tick();
        (next === undefined ? root.querySelector<HTMLElement>(`[data-year="${from}"]`) : back)?.focus();
    }
    // In the capture phase and with no default, so that the window stays open.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || year === undefined || only !== undefined) return;
        event.preventDefault();
        void show(undefined);
    }
</script>

<svelte:window onkeydowncapture={escape} />

{#snippet beside()}
    {#if year !== undefined && only === undefined}
        <button type="button" class="back" bind:this={back} onclick={() => show(undefined)}>
            <svg viewBox="0 0 14 14" aria-hidden="true"><path d="M8.5 3 4.5 7l4 4" /></svg>All years
        </button>
    {/if}
{/snippet}

{#snippet column(bar: typeof bars[number])}
    {@const value = amount(measure, bar.totals)}
    <span class="area">
        {#if value > 0}
            <span class="amount">{formatCount(value)}</span>
            <span class="stack" style:height={`calc((100% - 22px) * ${value / top})`}>
                {#each order.filter(bike => bar.kinds.has(bike)) as bike (bike)}
                    <span style:flex-grow={amount(measure, bar.kinds.get(bike)!)} style:background={`var(--kind-${order.indexOf(bike)})`}></span>
                {/each}
            </span>
        {/if}
    </span>
    <span class="name">{bar.name}</span>
{/snippet}

{#snippet record(label: string, value: string, ride: RideRecord, close: () => void)}
    <a href={`${ROUTES.ride}/${ride.key}`} onclick={close}>
        <span class="label">{label}</span>
        <strong>{value}</strong>
        <span class="of">{ride.name} · {dayText(ride.summary.start * 1000)}</span>
    </a>
{/snippet}

<Window title="Statistics" wide {onClose} {beside}>
    {#snippet children(close)}
        <div class="statistics" bind:this={root}>
            <div class="scope">
                <h3>{shownYear ?? 'All years'}</h3>
                {#if both}
                    <Segmented compact label="By bike or on foot" value={walks ? 'foot' : 'bike'}
                        options={[{ value: 'bike', label: 'By bike' }, { value: 'foot', label: 'On foot' }]}
                        onChange={next => { foot = next === 'foot'; year = undefined; }} />
                {/if}
            </div>

            <!-- The four numbers choose the measure of the bars. -->
            <div class="numbers" role="radiogroup" aria-label="The measure of the bars">
                {#each MEASURES as key, index (key)}
                    <button type="button" role="radio" aria-checked={measure === key} tabindex={measure === key ? 0 : -1}
                        onclick={() => measure = key} onkeydown={event => radioKey(event, index, MEASURES.length, next => measure = MEASURES[next])}>
                        <span class="label">{labels[key]}</span>
                        <span class="value">{text(key, totals)}</span>
                    </button>
                {/each}
            </div>

            <div class="middle">
                <ol class="bars" class:alone={walks || order.length < 2} onfocusout={() => active = undefined} aria-label={`${labels[measure]} of each ${shownYear === undefined ? 'year' : 'month'}`}>
                    {#each bars as bar (bar.name)}
                        {@const title = `${bar.name}: ${text(measure, bar.totals)}`}
                        <li>
                            {#if bar.totals.count}
                                <!-- A year opens its months. A month only shows its number, which a touch screen gets with a tap. -->
                                <button type="button" class="column" class:active={active === bar.name} data-year={bar.year} {title} aria-label={title}
                                    onclick={() => bar.year === undefined ? active = bar.name : show(bar.year)} onfocus={() => active = bar.name}
                                    onpointerenter={() => active = bar.name}
                                    onpointerleave={event => { if (event.pointerType === 'mouse') active = undefined; }}>{@render column(bar)}</button>
                            {:else}
                                <span class="column">{@render column(bar)}</span>
                            {/if}
                        </li>
                    {/each}
                </ol>

                {#if !walks && order.length > 1}
                    <table>
                        <thead>
                            <tr><td></td><th scope="col">{settings.units}</th><th scope="col">{heightUnit()} up</th><th scope="col">rides</th></tr>
                        </thead>
                        <tbody>
                            {#each order as bike, index (bike)}
                                {@const of = kinds.get(bike)}
                                <tr class:none={!of}>
                                    <th scope="row"><span class="dot" style:background={`var(--kind-${index})`}></span>{ridingProfiles[bike].label}</th>
                                    <td>{of ? formatCount(Math.round(distanceIn(of.lengthM))) : '–'}</td>
                                    <td>{of ? formatCount(Math.round(heightIn(of.climbM))) : '–'}</td>
                                    <td>{of ? formatCount(of.count) : '–'}</td>
                                </tr>
                            {/each}
                        </tbody>
                    </table>
                {/if}
            </div>

            <div class="largest">
                {#if best.ride}{@render record(walks ? 'Longest hike' : 'Longest ride', formatDistance(best.ride.summary.lengthM), best.ride, close)}{/if}
                {#if best.day}{@render record('Most climb in one day', formatHeight(best.day.climbM), best.day.ride, close)}{/if}
                {#if best.trip}
                    {@const { trip, rides: own, lengthM, days } = best.trip}
                    <button type="button" onclick={() => { close(); onTrip(trip.id); }}>
                        <span class="label">Longest trip</span>
                        <strong>{formatDistance(lengthM)} · {days} {days === 1 ? 'day' : 'days'}</strong>
                        <span class="of">{trip.name} · {dayText(own[0].summary.start * 1000, { month: 'short', year: 'numeric' })}</span>
                    </button>
                {/if}
                {#if best.highest}{@render record('Highest point', formatHeight(best.highest.summary.highestM!), best.highest, close)}{/if}
            </div>
        </div>
    {/snippet}
</Window>

<style>
    /* The kinds of bike are four near tones, the ride colour first: one family, so the bars stay quiet. */
    .statistics {
        --kind-0: var(--ride);
        --kind-1: var(--water);
        --kind-2: var(--wood);
        --kind-3: color-mix(in srgb, var(--wood) 40%, var(--panel));
        display: grid;
        gap: 28px;
        padding: 4px 0 8px;
    }
    .scope {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        min-height: 30px;
    }
    h3 {
        margin: 0;
        font: 600 15px/1.3 var(--sans);
    }
    .back {
        display: inline-flex;
        gap: 4px;
        align-items: center;
        padding: 4px 10px 4px 6px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 13px/16px var(--sans);
        white-space: nowrap;
    }
    .back:hover {
        background: var(--parchment-2);
    }
    .back svg {
        width: 14px;
        height: 14px;
        fill: none;
        stroke: currentColor;
        stroke-width: 1.4;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    .numbers, .largest {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 16px 24px;
    }
    /* A number or a largest value has its ground only under the pointer, and its text starts on the edge of the window's text. */
    .numbers button, .largest :is(a, button) {
        display: flex;
        flex-direction: column;
        gap: 2px;
        margin: -8px -10px;
        padding: 8px 10px;
        border: 0;
        border-radius: 8px;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
        text-decoration: none;
    }
    .numbers button:hover, .largest :is(a, button):hover, .column[data-year]:hover {
        background: var(--parchment);
    }
    .label, .of {
        color: var(--ink-soft);
        font-size: 13px;
    }
    /* Only the measure of the bars is in ink: one number leads. */
    .value {
        color: var(--ink-soft);
        font: 600 26px/1.2 var(--sans);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    [aria-checked="true"] :is(.label, .value) {
        color: var(--ink);
    }
    [aria-checked="true"] .value {
        font-weight: 700;
    }
    /* A short line under the number that the bars show. */
    .numbers button::after {
        width: 20px;
        height: 2px;
        margin-top: 6px;
        border-radius: 1px;
        content: '';
    }
    .numbers [aria-checked="true"]::after {
        background: var(--ink);
    }
    .middle {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 24px;
        align-items: end;
    }
    .bars {
        display: flex;
        margin: 0;
        padding: 0;
        list-style: none;
    }
    .alone {
        grid-column: 1 / -1;
    }
    .bars li {
        flex: 1 1 0;
        min-width: 0;
        max-width: 88px;
        container-type: inline-size;
    }
    .column {
        display: flex;
        flex-direction: column;
        width: 100%;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: inherit;
    }
    .column:not([data-year]) {
        cursor: default;
    }
    .active {
        color: var(--ink);
    }
    .area {
        display: flex;
        flex-direction: column;
        align-items: center;
        justify-content: flex-end;
        height: 200px;
        border-bottom: 1px solid var(--line);
    }
    .amount {
        margin-bottom: 4px;
        font-size: 11.5px;
        line-height: 18px;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .stack {
        display: flex;
        flex-direction: column-reverse;
        gap: 1px;
        width: 62%;
        max-width: 44px;
        min-height: 2px;
        overflow: hidden;
        border-radius: 3px 3px 0 0;
    }
    .stack span {
        flex: 0 0 0;
    }
    .name {
        padding: 6px 0 5px;
        font-size: 12.5px;
        text-align: center;
    }
    /* The numbers of narrow bars would run into each other, so only the active bar shows its number. */
    @container (max-width: 34px) {
        .column:not(.active) .amount {
            visibility: hidden;
        }
    }
    @media (prefers-reduced-motion: no-preference) {
        .stack {
            transition: height 240ms cubic-bezier(.2, .8, .2, 1);
        }
    }
    /* The last row of the table stands on the line of the bars. */
    table {
        width: 100%;
        max-width: 360px;
        margin-bottom: 29px;
        border-collapse: collapse;
        font-size: 13.5px;
    }
    th, td {
        padding: 5px 0 5px 16px;
        color: var(--ink-soft);
        font-weight: 400;
        font-variant-numeric: tabular-nums;
        text-align: right;
        white-space: nowrap;
    }
    thead th {
        padding-bottom: 8px;
        font-size: 12.5px;
    }
    tbody th {
        padding-left: 0;
        color: var(--ink);
        text-align: left;
    }
    .none :is(th, td) {
        color: var(--ink-faint);
    }
    .dot {
        display: inline-block;
        width: 10px;
        height: 10px;
        margin-right: 8px;
        border-radius: 2px;
    }
    .largest {
        padding-top: 28px;
        border-top: 1px solid var(--line);
    }
    .largest strong {
        font: 600 17px/1.3 var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .of {
        overflow-wrap: anywhere;
    }
    @media (max-width: 760px) {
        .statistics {
            gap: 24px;
        }
        .numbers {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
        .value {
            font-size: 22px;
        }
        .middle, .largest {
            grid-template-columns: minmax(0, 1fr);
        }
        .area {
            height: 160px;
        }
        table {
            margin-bottom: 0;
        }
    }
</style>
