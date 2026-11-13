<script lang="ts">
    import Card from '../../lib/ui/Card.svelte';
    import Lanes from './Lanes.svelte';
    import Plot from './Plot.svelte';
    import RideLarge from './RideLarge.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import { townOf } from './towns';
    import type { FitSample } from '../../lib/core/fit';
    import { duration } from '../../lib/planner/editor';
    import { distanceAxis, extent, outOfStops, range, sampleAt, smooth, speeds, stops, thin, timeAxis } from '../../lib/ride/lines';
    import type { Channels, PartNumbers, RideClimb } from '../../lib/ride/numbers';
    import { formatAlong, formatDistance, formatHeight, formatSpeed } from '../../lib/settings/format';

    /** The bottom card of the Ride page: one row for each line of the ride on its axis, with the average of the
     * line under its name and its scale after the row. A line with no value has no row. A drag selects a part; a click
     * on a climb in the elevation row selects the climb. A click on the label of a row opens its line large in a window.
     * The name of the axis beside the ticks is its switch: the distance, or the time of day, where a stop has a width and
     * the lines end at a gap of the recording. A stop is a mark on the row of the ticks; the foot of the pointer says
     * its length. */
    let { samples, shares, speed, channels, climbs = [], mark, onpoint, part, onpart, partFacts }: {
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
        /** The numbers of the selected part, which the large line shows. */
        partFacts?: PartNumbers;
    } = $props();

    type Key = 'elevation' | 'speed' | 'hr' | 'power' | 'cadence';
    /** The lines are smoothed over SMOOTH_S. A row is ROW_PX high, and lower in a low window, down to LOW_ROW_PX, so that
     * the cards over the rows keep CARDS_PX: the page around the cards and the rows is about PAGE_PX high, the card
     * around the rows about FRAME_PX, and GAP_PX are between two rows. */
    const SMOOTH_S = 10, ROW_PX = 56, LOW_ROW_PX = 36, CARDS_PX = 340, PAGE_PX = 176, FRAME_PX = 92, GAP_PX = 8;
    /** The mark of a stop is at least STOP_PX wide, and the pointer is on a stop within that width. A tick of the time
     * axis has about CLOCK_PX. */
    const STOP_PX = 6, CLOCK_PX = 90;

    const whole = (unit: string) => (value: number) => `${Math.round(value)} ${unit}`;
    /** `unit` and `limit` are of a line with zones (`ZoneTimes`). */
    const series: Record<Key, { label: string; color: string; format: (value: number) => string; unit?: string; limit?: string }> = {
        elevation: { label: 'Elevation', color: 'var(--ink-faint)', format: formatHeight },
        speed: { label: 'Speed', color: 'var(--speed)', format: formatSpeed },
        hr: { label: 'Heart rate', color: 'var(--heart-rate)', format: whole('bpm'), unit: 'bpm', limit: 'maximum heart rate' },
        power: { label: 'Power', color: 'var(--water)', format: whole('W'), unit: 'W', limit: 'threshold power' },
        cadence: { label: 'Cadence', color: 'var(--cadence)', format: whole('rpm') },
    };
    const keys = Object.keys(series) as Key[];

    let on = $state<Record<Key, boolean>>({ elevation: true, speed: true, hr: true, power: true, cadence: false });
    let width = $state(0), innerHeight = $state(900), start = $state('Start');
    let over = $state<{ x: number; row?: number | null } | null>(null);
    /** The line that shows large. */
    let large = $state<Key | null>(null);

    const axis = $derived(distanceAxis(samples));
    /** The axis is the time of day; the choice lasts as long as the page shows the ride. */
    let byTime = $state(false);
    const time = $derived(timeAxis(samples));
    const xs = $derived(byTime ? time.xs : axis.xs);
    const found = $derived(stops(samples).map(stop => ({ ...stop, from: xs[stop.first], to: xs[stop.last] })));
    const clock = (seconds: number) => new Date(seconds * 1000).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
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
    const columns = $derived(Object.fromEntries(shown.map(key => [key, thin(xs, values[key], Math.max(1, Math.round(width)), byTime ? time.gaps : [])])) as Partial<Record<Key, (number | null)[]>>);
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

    /** The elevation row fills the stretch of each climb, as the profile of the Route page. */
    const stretches = $derived(climbs.map(climb => ({ from: xs[climb.first], to: xs[climb.last] })));
    /** The climb that is the selected part. */
    const chosen = $derived(part ? climbs.findIndex(climb => climb.first === part[0] && climb.last === part[1]) : -1);
    /** The pointer on the axis: its place in the rows when the rows gave `mark`, so that a key step shorter than the
     * gap between two samples moves it; else the sample of `mark`. */
    const pointer = $derived(mark === undefined ? undefined
        : over && shares[sampleAt(xs, over.x)] === mark ? over.x : xs[sampleAt(shares, mark)]);
    const shownPart = $derived<[number, number] | null>(part && [xs[part[0]], xs[part[1]]]);
    /** The ends of the part by the distance or by the time of day, then its distance. */
    const partRange = (times: boolean) => {
        if (!part) return '';
        const [from, to] = [axis.xs[part[0]] * axis.lengthM, axis.xs[part[1]] * axis.lengthM];
        return `${times ? `${clock(samples[part[0]].timeS)} to ${clock(samples[part[1]].timeS)}` : `${formatAlong(from)} to ${formatAlong(to)}`} · ${formatDistance(to - from)}`;
    };
    const partText = $derived(partRange(byTime));
    /** The start, the end, then round distances; on the time axis the time of the start and of the end, then whole
     * times of the day. */
    const ticks = $derived.by(() => {
        if (!byTime) return fitTicks([{ at: 0, label: start }, { at: 1, label: formatDistance(axis.lengthM) }, ...distanceTicks(0, axis.lengthM, width)], width);
        const first = samples[0].timeS, seconds = samples.at(-1)!.timeS - first || 1, day = new Date(first * 1000);
        const step = [1, 5, 10, 15, 30, 60, 120, 240].find(minutes => seconds / 60 / minutes <= Math.max(1, width / CLOCK_PX)) ?? 480;
        const minute = day.getHours() * 60 + day.getMinutes() + day.getSeconds() / 60, whole = [];
        for (let m = Math.ceil(minute / step) * step; (m - minute) * 60 < seconds; m += step) whole.push({ at: (m - minute) * 60 / seconds, label: clock(first + (m - minute) * 60) });
        return fitTicks([{ at: 0, label: clock(first) }, { at: 1, label: clock(first + seconds) }, ...whole], width);
    });
    /** The left edge and the width of the mark of a stop on the axis, in pixels: around the middle of the stop. */
    const markOf = (stop: { from: number; to: number }) => {
        const wide = Math.max(STOP_PX, (stop.to - stop.from) * width);
        return { left: Math.max(0, Math.min(width - wide, (stop.from + stop.to) / 2 * width - wide / 2)), wide };
    };
    /** The stop under the pointer. */
    const stopAt = $derived.by(() => {
        if (pointer === undefined) return undefined;
        const at = pointer * width;
        return found.find(stop => { const { left, wide } = markOf(stop); return at >= left && at <= left + wide; });
    });
    /** The foot of the pointer: its place, and the length of a stop under it. */
    const foot = $derived.by(() => {
        if (pointer === undefined) return '';
        const place = byTime ? clock(samples[sampleAt(xs, pointer)].timeS) : formatDistance(pointer * axis.lengthM);
        if (!stopAt) return place;
        return `${place} · stop ${stopAt.seconds < 3600 ? `${Math.round(stopAt.seconds / 60)} min` : duration(stopAt.seconds / 3600)}`;
    });
    const rowSnippets = { elevation: elevationRow, speed: speedRow, hr: hrRow, power: powerRow, cadence: cadenceRow };
    const rows = $derived(shown.map(key => ({
        label: series[key].label, note: averages[key], color: series[key].color, draw: rowSnippets[key], height: rowHeight,
        scale: ranges[key] && [series[key].format(ranges[key].high), series[key].format(ranges[key].low)] as [string, string],
        open: { label: `${series[key].label}, open large`, run: () => { large = key; } },
    })));

    /** The sensor of the large line and the numbers under it: of the selected part, else of the whole ride. */
    const shownLarge = $derived.by(() => {
        if (!large) return undefined;
        const key = large, { format } = series[key], selected = part && partFacts;
        const whole = key === 'hr' || key === 'power' || key === 'cadence' ? channels?.[key] : undefined;
        const sensor = whole && selected ? partFacts.channels[key as keyof Channels] : whole;
        // The smallest and largest value of the line as it is drawn, not the range of the row, which is wider for a flat
        // line. For the speed it is the largest mean over SMOOTH_S, as the scale of its row.
        const found = extent(selected ? values[key].slice(part[0], part[1] + 1) : values[key]);
        const text = (value?: number) => value === undefined ? '–' : format(value);
        const numbers = key === 'elevation' ? [{ label: 'Lowest', value: text(found?.low) }, { label: 'Highest', value: text(found?.high) }]
            : key === 'speed' ? [{ label: 'Average', value: text(selected ? partFacts.speed : speed) }, { label: 'Highest', value: text(found?.high) }]
                : [{ label: 'Average', value: text(sensor?.mean) }, { label: 'Highest', value: text(sensor?.max) }];
        // The zones of the line are those of the whole ride, also when the part has no value of the sensor. A sensor
        // with no limit keeps its link to the Settings then.
        return { key, times: sensor ?? (whole?.zones ? undefined : whole), numbers, zones: whole?.zones?.map(({ zone }) => zone) };
    });

    /** The samples of a part of the axis. On the distance axis a stop is one place: a part does not begin or end in
     * it, so the time of the stop is in no part by chance. */
    const partOf = ([from, to]: [number, number]): [number, number] => {
        const ends: [number, number] = [sampleAt(xs, from), sampleAt(xs, to)];
        return byTime ? ends : outOfStops(found, ends);
    };

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
        climbs={key === 'elevation' ? stretches : []}
        lit={chosen >= 0 ? chosen : undefined} hover={key === 'elevation' && hovered >= 0 ? hovered : undefined} mark={pointer} format={series[key].format} bind:width />
{/snippet}
{#snippet elevationRow()}{@render plot('elevation')}{/snippet}
{#snippet speedRow()}{@render plot('speed')}{/snippet}
{#snippet hrRow()}{@render plot('hr')}{/snippet}
{#snippet powerRow()}{@render plot('power')}{/snippet}
{#snippet cadenceRow()}{@render plot('cadence')}{/snippet}

{#snippet chip()}{foot}{/snippet}

{#snippet marks()}
    {#each found as stop (stop.first)}
        {@const { left, wide } = markOf(stop)}
        <b class="stop" style:left={`${left}px`} style:width={`${wide}px`}></b>
    {/each}
{/snippet}

{#snippet corner()}
    <button type="button" class="axis" aria-label={byTime ? 'Time axis, change to distance' : 'Distance axis, change to time'} onclick={() => { byTime = !byTime; }}>
        {byTime ? 'Time' : 'Distance'}<svg viewBox="0 0 14 14" aria-hidden="true"><path d="M2 5h9M8.5 2.5 11 5 8.5 7.5M12 9.5H3M5.5 7 3 9.5 5.5 12" /></svg>
    </button>
{/snippet}

{#snippet header()}
    <div class="chips" role="group" aria-label="Shown lines">
        {#each present as key (key)}
            <button type="button" aria-pressed={on[key]} onclick={() => on[key] = !on[key]}>{series[key].label}</button>
        {/each}
    </div>
{/snippet}

{#if shownLarge}
    {@const { key, times, numbers, zones } = shownLarge}
    <RideLarge title={series[key].label} color={series[key].color} format={series[key].format} xs={axis.xs} lengthM={axis.lengthM} values={values[key]}
        {zones} times={series[key].limit ? times : undefined} unit={series[key].unit} missing={series[key].limit} grades={key === 'elevation'}
        part={part && [axis.xs[part[0]], axis.xs[part[1]]]} partText={partRange(false)} {numbers} {start} onClose={() => large = null} />
{/if}

<div class="lines" style:cursor={hovered >= 0 ? 'pointer' : undefined}>
    <Card>
        <Lanes label="Lines of the ride" {rows} {ticks} {header} {pointer} {chip} part={shownPart} {partText} onclick={click} {corner} {marks}
            onpointer={(x, row) => { over = x === null ? null : { x, row }; onpoint(x === null ? null : shares[sampleAt(xs, x)]); }}
            onpart={next => onpart(next && partOf(next))} />
    </Card>
</div>

<style>
    /* The name of the axis looks as the label of a row, and is a button as the label of a row is. */
    .axis {
        display: flex;
        gap: 5px;
        align-items: center;
        margin: -3px -6px;
        padding: 3px 6px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font-size: 12px;
        line-height: 16px;
    }
    .axis svg {
        width: 11px;
        height: 11px;
        fill: none;
        stroke: var(--ink-faint);
        stroke-width: 1.4;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    .axis:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .axis:hover svg {
        stroke: var(--ink);
    }
    /* On the axis, over the ticks. */
    .stop {
        position: absolute;
        top: -5px;
        height: 3px;
        border-radius: 2px;
        background: var(--ink-soft);
    }
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
