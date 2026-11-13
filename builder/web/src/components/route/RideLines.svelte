<script lang="ts">
    import Card from '../../lib/ui/Card.svelte';
    import Lanes from './Lanes.svelte';
    import Plot from './Plot.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import { townOf } from './towns';
    import type { FitSample } from '../../lib/core/fit';
    import { distanceAxis, range, sampleAt, smooth, speeds, thin } from '../../lib/ride/lines';
    import type { Channels, RideClimb } from '../../lib/ride/numbers';
    import { formatAlong, formatDistance, formatHeight, formatSpeed } from '../../lib/settings/format';

    /** The bottom card of the Ride page: one row for each line of the ride on its distance axis, with the average of the
     * line under its name and its scale after the row. A line with no value has no row. A drag selects a part; a click
     * on a climb in the elevation row selects the climb. */
    let { samples, shares, speed, channels, climbs = [], mark, onpoint, part, onpart }: {
        samples: FitSample[];
        /** The share of the summary line at each sample (`lineShares`). */
        shares: number[];
        /** The averages that the left card shows: the moving speed in m/s, and the sensors. */
        speed?: number;
        channels?: Channels;
        climbs?: RideClimb[];
        /** The pointer, as a share of the summary line. */
        mark?: number;
        onpoint: (share: number | null) => void;
        /** The selected part, from its first to its last sample. */
        part: [number, number] | null;
        onpart: (part: [number, number] | null) => void;
    } = $props();

    type Key = 'elevation' | 'speed' | 'hr' | 'power' | 'cadence';
    /** The lines are smoothed over SMOOTH_S. A row is ROW_PX high, and lower in a low window, down to LOW_ROW_PX, so that
     * the cards over the rows keep CARDS_PX: the page around the cards and the rows is about PAGE_PX high, the card
     * around the rows about FRAME_PX, and GAP_PX are between two rows. */
    const SMOOTH_S = 10, ROW_PX = 56, LOW_ROW_PX = 36, CARDS_PX = 340, PAGE_PX = 176, FRAME_PX = 92, GAP_PX = 8;
    /** The elevation row has the grade colours of the profile of the Route page. */
    const ELEVATION_GRADES = false;
    /** The elevation row fills the stretch of each climb, as the profile of the Route page. */
    const ELEVATION_CLIMBS = true;

    const whole = (unit: string) => (value: number) => `${Math.round(value)} ${unit}`;
    const series: Record<Key, { label: string; color: string; format: (value: number) => string }> = {
        elevation: { label: 'Elevation', color: 'var(--ink-faint)', format: formatHeight },
        speed: { label: 'Speed', color: 'var(--speed)', format: formatSpeed },
        hr: { label: 'Heart rate', color: 'var(--ride)', format: whole('bpm') },
        power: { label: 'Power', color: 'var(--water)', format: whole('W') },
        cadence: { label: 'Cadence', color: 'var(--cadence)', format: whole('rpm') },
    };
    const keys = Object.keys(series) as Key[];

    let on = $state<Record<Key, boolean>>({ elevation: true, speed: true, hr: true, power: true, cadence: false });
    let width = $state(0), innerHeight = $state(900), start = $state('Start');
    let over = $state<{ x: number; row?: number | null } | null>(null);

    const axis = $derived(distanceAxis(samples));
    const values = $derived<Record<Key, (number | undefined)[]>>({
        elevation: samples.map(sample => sample.elevationM),
        speed: smooth(samples, speeds(samples), SMOOTH_S),
        hr: smooth(samples, samples.map(sample => sample.hrBpm), SMOOTH_S),
        power: smooth(samples, samples.map(sample => sample.powerW), SMOOTH_S),
        // A cadence of 0 is coasting, as in the average.
        cadence: smooth(samples, samples.map(sample => sample.cadenceRpm || undefined), SMOOTH_S),
    });
    const present = $derived(keys.filter(key => values[key].some(value => value !== undefined)));
    const shown = $derived(present.filter(key => on[key]));
    const columns = $derived(Object.fromEntries(shown.map(key => [key, thin(axis.xs, values[key], Math.max(1, Math.round(width)))])) as Partial<Record<Key, (number | null)[]>>);
    // From the smoothed values, not from the columns: the scale of a row does not change with the width of the window.
    const ranges = $derived(Object.fromEntries(present.map(key => [key, range(values[key]) ?? { low: 0, high: 1 }])) as Partial<Record<Key, { low: number; high: number }>>);
    const rowHeight = $derived(Math.max(LOW_ROW_PX, Math.min(ROW_PX,
        Math.floor((innerHeight - PAGE_PX - CARDS_PX - FRAME_PX - (shown.length - 1) * GAP_PX) / Math.max(1, shown.length)))));
    const averages = $derived<Partial<Record<Key, string>>>({
        ...speed !== undefined && { speed: formatSpeed(speed) },
        ...channels?.hr && { hr: series.hr.format(channels.hr.mean) },
        ...channels?.power && { power: series.power.format(channels.power.mean) },
        ...channels?.cadence && { cadence: series.cadence.format(channels.cadence.mean) },
    });

    const stretches = $derived(ELEVATION_CLIMBS ? climbs.map(climb => ({ from: axis.xs[climb.first], to: axis.xs[climb.last] })) : []);
    /** The climb that is the selected part. */
    const chosen = $derived(part ? climbs.findIndex(climb => climb.first === part[0] && climb.last === part[1]) : -1);
    const pointer = $derived(mark === undefined ? undefined : axis.xs[sampleAt(shares, mark)]);
    const shownPart = $derived<[number, number] | null>(part && [axis.xs[part[0]], axis.xs[part[1]]]);
    const partText = $derived(shownPart ? `${formatAlong(shownPart[0] * axis.lengthM)} to ${formatAlong(shownPart[1] * axis.lengthM)} · ${formatDistance((shownPart[1] - shownPart[0]) * axis.lengthM)}` : '');
    const ticks = $derived(fitTicks([{ at: 0, label: start }, { at: 1, label: formatDistance(axis.lengthM) }, ...distanceTicks(0, axis.lengthM, width)], width));
    const rowSnippets = { elevation: elevationRow, speed: speedRow, hr: hrRow, power: powerRow, cadence: cadenceRow };
    const rows = $derived(shown.map(key => ({
        label: series[key].label, note: averages[key], color: series[key].color, draw: rowSnippets[key], height: rowHeight,
        scale: ranges[key] && [series[key].format(ranges[key].high), series[key].format(ranges[key].low)] as [string, string],
    })));

    /** The climb at a place of the elevation row, or of the keys, which have no row. */
    const climbAt = (x: number, row?: number | null) =>
        row === null || shown[row ?? -1] === 'elevation' ? stretches.findIndex(climb => x >= climb.from && x <= climb.to) : -1;
    /** The climb that a click selects: it is a little darker under the pointer. */
    const hovered = $derived(over ? climbAt(over.x, over.row) : -1);

    /** A click on a climb in the elevation row, or Enter, selects the climb; another click clears the part. */
    function click(x: number, row?: number | null) {
        const index = climbAt(x, row);
        onpart(index >= 0 ? [climbs[index].first, climbs[index].last] : null);
    }

    $effect(() => {
        const first = samples[0];
        let live = true;
        start = 'Start';
        void townOf([first.lon / 1e6, first.lat / 1e6]).then(name => { if (live && name) start = name; });
        return () => { live = false; };
    });
</script>

<svelte:window bind:innerHeight />

{#snippet plot(key: Key)}
    <Plot columns={columns[key] ?? []} low={ranges[key]?.low ?? 0} high={ranges[key]?.high ?? 1} color={series[key].color} area={key === 'elevation'}
        lengthM={key === 'elevation' && ELEVATION_GRADES ? axis.lengthM : undefined} climbs={key === 'elevation' ? stretches : []}
        lit={chosen >= 0 ? chosen : undefined} hover={key === 'elevation' && hovered >= 0 ? hovered : undefined} mark={pointer} format={series[key].format} bind:width />
{/snippet}
{#snippet elevationRow()}{@render plot('elevation')}{/snippet}
{#snippet speedRow()}{@render plot('speed')}{/snippet}
{#snippet hrRow()}{@render plot('hr')}{/snippet}
{#snippet powerRow()}{@render plot('power')}{/snippet}
{#snippet cadenceRow()}{@render plot('cadence')}{/snippet}

{#snippet chip()}{pointer === undefined ? '' : formatDistance(pointer * axis.lengthM)}{/snippet}

{#snippet header()}
    <div class="chips" role="group" aria-label="Shown lines">
        {#each present as key (key)}
            <button type="button" aria-pressed={on[key]} onclick={() => on[key] = !on[key]}>{series[key].label}</button>
        {/each}
    </div>
{/snippet}

<div class="lines" style:cursor={hovered >= 0 ? 'pointer' : undefined}>
    <Card>
        <Lanes label="Lines of the ride" {rows} {ticks} {header} {pointer} {chip} part={shownPart} {partText} onclick={click}
            onpointer={(x, row) => { over = x === null ? null : { x, row }; onpoint(x === null ? null : shares[sampleAt(axis.xs, x)]); }}
            onpart={next => onpart(next && [sampleAt(axis.xs, next[0]), sampleAt(axis.xs, next[1])])} />
    </Card>
</div>

<style>
    .chips {
        display: flex;
        flex-wrap: wrap;
        gap: 4px;
    }
    /* A line that shows is a box; a hidden one is plain text. */
    .chips button {
        padding: 3px 9px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .chips button:hover {
        color: var(--ink);
    }
    .chips button[aria-pressed="true"] {
        background: var(--parchment-2);
        color: var(--ink);
    }
    /* On the dark panel, `--parchment-2` does not show as a box. */
    :global(:root[data-theme="dark"]) .chips button[aria-pressed="true"] {
        background: var(--parchment-3);
    }
</style>
