<script lang="ts">
    import { gradeBands, gradePaths, profileGrades } from '../../lib/planner/grade-data';
    import { profilePaths } from '../../lib/planner/profile-data';
    import { zoneStretches } from '../../lib/ride/lines';
    import { zoneOf, type Zone } from '../../lib/settings/zones';
    import { appearance } from '../../lib/theme.svelte';
    import ClimbFills from './ClimbFills.svelte';

    /** One line of a ride over the width of its lane, from one value for each pixel column: `low` is the floor of the
     * box and `high` stands just under its top. With `area`, the ground under the line is filled. `climbs` are
     * stretches of the axis that the area fills darker, as the profile of the Route page fills its climbs. `mark` is a
     * place on the axis from 0 to 1: a dot on the line, and its value beside the pointer. The line is `--plot-stroke`
     * wide. */
    let { columns, low, high, color, area = false, climbs = [], lit, hover, zones, grades, mark, format, width = $bindable(0) }: {
        columns: (number | null)[];
        low: number;
        high: number;
        color: string;
        area?: boolean;
        climbs?: { from: number; to: number }[];
        /** The index of the climb that is darker. */
        lit?: number;
        /** The index of the climb under the pointer. */
        hover?: number;
        /** Each stretch of the line has the colour of its zone, and a thin rule with the name stands where a zone starts. */
        zones?: Zone[];
        /** The length of the axis in metres: the line has the colour of the steepness band of each stretch, as the
         * profile of the Route page. */
        grades?: number;
        mark?: number;
        format: (value: number) => string;
        width?: number;
    } = $props();

    /** The pointer nearer than FLIP to the end of the axis has its value on its left. */
    const FLIP = .85;
    const samples = $derived(columns.map((height, i) => ({ progress: (i + .5) / columns.length, height })));
    const paths = $derived(profilePaths(samples, low, high));
    /** The row of a value in the box of `profilePaths`: 105 high, with `high` at 5. */
    const y = (value: number) => 105 - (value - low) / (high - low) * 100;
    /** One path for each colour of the line. */
    const lines = $derived.by((): { d: string; stroke: string }[] => {
        if (grades) {
            return gradePaths(samples, profileGrades(samples, grades / 1000), low, high)
                .map((d, band) => ({ d, stroke: appearance.dark ? gradeBands[band].dark : gradeBands[band].color })).filter(line => line.d);
        }
        if (!zones) return [{ d: paths.line, stroke: color }];
        const at = (i: number) => `${(i + .5) / columns.length * 1000} ${y(columns[i]!)}`, found = zones.map(() => '');
        for (const { zone, from, to } of zoneStretches(columns, zones)) {
            // A stretch ends at the first column of the next one, so the line stays joined.
            const end = columns[to + 1] == null ? to : to + 1;
            found[zone] += `M${at(from)}`;
            for (let i = from + 1; i <= end; i++) found[zone] += `L${at(i)}`;
        }
        return found.map((d, zone) => ({ d, stroke: `var(--zone-${zone + 1})` })).filter(line => line.d);
    });
    /** The lowest value of each zone that the line reaches. */
    const rules = $derived((zones ?? []).map((zone, index) => ({ zone, index })).filter(({ zone }) => zone.from > low && zone.from < high));
    const marked = $derived.by(() => {
        const value = mark === undefined ? null : columns[Math.min(columns.length - 1, Math.floor(mark * columns.length))];
        if (value == null) return undefined;
        return { value, top: y(value) / 105 * 100, color: zones ? `var(--zone-${zoneOf(zones, value) + 1})` : color };
    });
</script>

<div class="plot" bind:clientWidth={width}>
    <svg viewBox="0 0 1000 105" preserveAspectRatio="none" aria-hidden="true">
        {#if area}
            <path class="area" d={paths.area} />
            <ClimbFills area={paths.area} climbs={climbs.map(({ from, to }, index) => ({ a: from, b: to, index }))} {lit} {hover} />
        {/if}
        {#each lines as line (line.stroke)}<path class="line" d={line.d} style:stroke={line.stroke} />{/each}
    </svg>
    {#each rules as { zone, index } (index)}
        <span class="rule" style:top={`${y(zone.from) / 105 * 100}%`}><span><i style:background={`var(--zone-${index + 1})`}></i>{zone.name} from {format(zone.from)}</span></span>
    {/each}
    {#if marked && mark !== undefined}
        <b style:left={`${mark * 100}%`} style:top={`${marked.top}%`} style:background={marked.color}></b>
        <span class="value" class:flip={mark > FLIP} style:left={`${mark * 100}%`}>{format(marked.value)}</span>
    {/if}
</div>

<style>
    .plot {
        position: relative;
        height: 100%;
    }
    svg {
        display: block;
        width: 100%;
        height: 100%;
    }
    .area {
        fill: color-mix(in srgb, var(--ink-faint) 10%, var(--panel));
    }
    .line {
        fill: none;
        stroke-width: var(--plot-stroke, 1.5);
        stroke-linejoin: round;
        vector-effect: non-scaling-stroke;
    }
    b {
        position: absolute;
        box-sizing: content-box;
        width: 7px;
        height: 7px;
        border: 2px solid var(--panel);
        border-radius: 50%;
        translate: -50% -50%;
    }
    .rule {
        position: absolute;
        right: 0;
        left: 0;
        display: flex;
        justify-content: flex-end;
        border-top: 1px solid var(--line);
        color: var(--ink-faint);
        font-size: 11px;
        line-height: 13px;
        font-variant-numeric: tabular-nums;
        pointer-events: none;
    }
    /* On a plate of the panel, so the line does not cross the name of a zone. */
    .rule span {
        display: flex;
        gap: 5px;
        align-items: center;
        padding: 3px 0 2px 8px;
        border-radius: 0 0 0 5px;
        background: var(--panel);
    }
    .rule i {
        width: 7px;
        height: 7px;
        border-radius: 50%;
    }
    /* On a plate of the panel, so the line behind it does not cross the figures. */
    .value {
        position: absolute;
        top: 0;
        margin-left: 7px;
        padding: 0 4px;
        border-radius: 4px;
        background: var(--panel);
        font: 600 12px/16px var(--sans);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .value.flip {
        margin-left: -7px;
        translate: -100%;
    }
</style>
