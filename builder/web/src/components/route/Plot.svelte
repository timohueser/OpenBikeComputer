<script lang="ts">
    import { profilePaths } from '../../lib/planner/profile-data';
    import ClimbFills from './ClimbFills.svelte';

    /** One line of a ride over the width of its lane, from one value for each pixel column: `low` is the floor of the
     * box and `high` stands just under its top. With `area`, the ground under the line is filled. `climbs` are
     * stretches of the axis that the area fills darker, as the profile of the Route page fills its climbs. `mark` is a
     * place on the axis from 0 to 1: a dot on the line, and its value beside the pointer. */
    let { columns, low, high, color, area = false, climbs = [], lit, hover, mark, format, width = $bindable(0) }: {
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
        mark?: number;
        format: (value: number) => string;
        width?: number;
    } = $props();

    /** The pointer nearer than FLIP to the end of the axis has its value on its left. */
    const FLIP = .85;
    const samples = $derived(columns.map((height, i) => ({ progress: (i + .5) / columns.length, height })));
    const paths = $derived(profilePaths(samples, low, high));
    const marked = $derived.by(() => {
        const value = mark === undefined ? null : columns[Math.min(columns.length - 1, Math.floor(mark * columns.length))];
        // The box of `profilePaths` is 105 high, with `high` at 5.
        return value == null ? undefined : { value, top: (105 - (value - low) / (high - low) * 100) / 105 * 100 };
    });
</script>

<div class="plot" bind:clientWidth={width}>
    <svg viewBox="0 0 1000 105" preserveAspectRatio="none" aria-hidden="true">
        {#if area}
            <path class="area" d={paths.area} />
            <ClimbFills area={paths.area} climbs={climbs.map(({ from, to }, index) => ({ a: from, b: to, index }))} {lit} {hover} />
        {/if}
        <path class="line" d={paths.line} style:stroke={color} />
    </svg>
    {#if marked && mark !== undefined}
        <b style:left={`${mark * 100}%`} style:top={`${marked.top}%`} style:background={color}></b>
        <span class:flip={mark > FLIP} style:left={`${mark * 100}%`}>{format(marked.value)}</span>
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
        stroke-width: 1.5;
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
    /* On a plate of the panel, so the line behind it does not cross the figures. */
    span {
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
    span.flip {
        margin-left: -7px;
        translate: -100%;
    }
</style>
