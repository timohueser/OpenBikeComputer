<script lang="ts">
    import { columnDay, monthColumns } from '../../lib/planner/layers/data-layer';

    /** Wet days of 7 per week: the mean line, the band of most years, and the layer week marked. */
    let { mean, low, high, week, colors }: {
        mean: Float32Array;
        low: Float32Array;
        high: Float32Array;
        week: number;
        colors: { line: string; band: string };
    } = $props();

    const WEEKS = 52, DAYS = 7;
    const y = (days: number) => DAYS - Math.min(DAYS, Math.max(0, days));
    /** Runs of weeks with data, so a missing week leaves a gap. */
    const runs = $derived.by(() => {
        const list: number[][] = [];
        for (let w = 0; w < WEEKS; w++) {
            if (Number.isNaN(mean[w])) continue;
            if (w === 0 || Number.isNaN(mean[w - 1])) list.push([]);
            list.at(-1)!.push(w);
        }
        return list;
    });
    const band = $derived(runs.map(run => [...run.map(w => `${w + 0.5},${y(high[w])}`), ...[...run].reverse().map(w => `${w + 0.5},${y(low[w])}`)].join(' ')));
    const line = $derived(runs.map(run => run.map(w => `${w + 0.5},${y(mean[w])}`).join(' ')));
    const monthName = (column: number) => columnDay(column, WEEKS).toLocaleDateString('en-GB', { month: 'narrow', timeZone: 'UTC' });
</script>

<figure class="rain-weeks" role="img" aria-label="Wet days of 7 in each week of the year: the mean line and the band of most years, with your week marked">
    <ol class="axis" aria-hidden="true"><li>7 days</li><li>0</li></ol>
    <div class="plot">
        <svg viewBox="0 0 {WEEKS} {DAYS}" preserveAspectRatio="none" aria-hidden="true">
            <line class="grid" x1="0" x2={WEEKS} y1={DAYS / 2} y2={DAYS / 2} />
            {#each band as points, i (i)}<polygon {points} fill={colors.band} />{/each}
            {#each line as points, i (i)}<polyline {points} stroke={colors.line} />{/each}
            <line class="week" x1={week + 0.5} x2={week + 0.5} y1="0" y2={DAYS} />
        </svg>
        <ol class="months" aria-hidden="true">
            {#each monthColumns(WEEKS) as column (column)}<li style:left={`${column / WEEKS * 100}%`}>{monthName(column)}</li>{/each}
        </ol>
    </div>
    <ul class="key">
        <li><span class="mean" style:background={colors.line}></span>Mean</li>
        <li><span class="most" style:background={colors.band}></span>Most years</li>
        <li><span class="marker"></span>Your date</li>
    </ul>
</figure>

<style>
    .rain-weeks { display: grid; grid-template-columns: 48px minmax(0, 1fr); margin: 0; }
    .axis { display: flex; flex-direction: column; justify-content: space-between; height: 56px; margin: 0; padding: 0 6px 0 0; list-style: none; text-align: right; font-size: 11px; line-height: 1; color: var(--ink-soft); }
    .plot { position: relative; min-width: 0; padding-bottom: 16px; }
    svg { display: block; width: 100%; height: 56px; overflow: visible; border-bottom: 1px solid var(--line-strong); }
    polyline { fill: none; stroke-width: 2; stroke-linejoin: round; vector-effect: non-scaling-stroke; }
    .grid { stroke: var(--line); stroke-width: 1; stroke-dasharray: 2 3; vector-effect: non-scaling-stroke; }
    .week { stroke: var(--ink); stroke-width: 2; vector-effect: non-scaling-stroke; }
    .months { position: absolute; left: 0; right: 0; bottom: 0; height: 14px; margin: 0; padding: 0; list-style: none; }
    .months li { position: absolute; padding-left: 3px; border-left: 1px solid var(--line-strong); font-size: 11px; line-height: 14px; color: var(--ink-soft); }
    .key { grid-column: 2; display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 4px 0 0; padding: 0; list-style: none; font-size: 12px; color: var(--ink-soft); }
    .key li { display: flex; align-items: center; gap: 6px; }
    .mean { width: 14px; height: 2px; }
    .most { width: 14px; height: 8px; border-radius: 2px; }
    .marker { width: 2px; height: 12px; background: var(--ink); }
</style>
