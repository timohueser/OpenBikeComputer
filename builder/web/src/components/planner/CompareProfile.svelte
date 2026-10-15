<script lang="ts">
    import DrawerTitle from './DrawerTitle.svelte';
    import { heightRange, profilePaths, profileSamples } from '../../lib/planner/profile-data';
    import type { Compared } from '../../lib/planner/route-options';

    let { option, current, letter, height, open, onToggle }: {
        /** The previewed route, which the profile fills. */
        option: Compared;
        /** The rider's current route, letter A, as a dotted line. */
        current: Compared;
        letter: string;
        height: number;
        open: boolean;
        onToggle: () => void;
    } = $props();

    // One distance scale: the longer route spans the plot.
    const longest = $derived(Math.max(option.km, current.km) || 1);
    const optionSamples = $derived(profileSamples(option.line));
    const currentSamples = $derived(profileSamples(current.line));
    const { low, high } = $derived(heightRange(optionSamples, currentSamples));
    const optionPath = $derived(profilePaths(optionSamples, low, high, option.km / longest));
    const currentPath = $derived(profilePaths(currentSamples, low, high, current.km / longest));
    const ends = $derived([{ letter, row: option }, { letter: 'A', row: current }]);
</script>

<section class="elevation" style:height={`${height}px`} aria-label="Elevation profile">
    <div class="title">
        <DrawerTitle title="Elevation" {open} {onToggle} />
        {#if open}<span>Whole route · same scale for A and {letter}</span>{/if}
    </div>
    {#if open}
        <div class="plot">
            <span class="height" style:top="5%">{high} m</span>
            <span class="height" style:top="94%">{low} m</span>
            <svg viewBox="0 0 1000 112" preserveAspectRatio="none" role="img" aria-label={`Elevation of ${letter}, filled, and of your route A, dotted, on one distance scale.`}>
                <path d="M0 25H1000M0 65H1000M0 105H1000" class="grid" />
                <path d={optionPath.area} class="fill" />
                <path d={optionPath.line} class="option" />
                <path d={currentPath.line} class="current" />
            </svg>
            {#each ends as end, i (end.letter)}
                <span class="end" class:current={i === 1} style:left={`${end.row.km / longest * 100}%`} style:top={`${i * 18}px`}>{end.letter} {end.row.km.toFixed(1)} km</span>
            {/each}
        </div>
        <div class="axis"><span>0 km</span><span>{longest.toFixed(1)} km</span></div>
    {/if}
</section>

<style>
    .elevation {
        display: flex;
        flex-direction: column;
        flex: none;
        padding: 8px 24px;
        background: var(--panel);
    }
    .title {
        display: flex;
        align-items: center;
        gap: 12px;
        min-height: 28px;
        margin-bottom: 4px;
        font-size: 13px;
    }
    .title span {
        margin-left: auto;
        color: var(--ink-faint);
    }
    .plot {
        position: relative;
        flex: 1;
        min-height: 72px;
        margin: 0 12px 0 48px;
    }
    svg {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 100%;
    }
    path {
        fill: none;
        vector-effect: non-scaling-stroke;
    }
    .grid {
        stroke: var(--line);
    }
    .fill {
        fill: color-mix(in srgb, var(--ink) 10%, var(--panel));
    }
    .option {
        stroke: var(--ink);
        stroke-width: 2;
    }
    .current {
        stroke: var(--route);
        stroke-width: 2.5;
        stroke-dasharray: 1 4;
        stroke-linecap: round;
    }
    .height {
        position: absolute;
        left: -48px;
        transform: translateY(-50%);
        font-size: 11px;
        color: var(--ink-faint);
        font-variant-numeric: tabular-nums;
    }
    .end {
        position: absolute;
        transform: translateX(-100%);
        padding-right: 4px;
        font: 600 12px var(--sans);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .end.current {
        color: var(--ink-soft);
    }
    .axis {
        display: flex;
        justify-content: space-between;
        margin: 8px 12px 8px 48px;
        font-size: 11px;
        color: var(--ink-faint);
        font-variant-numeric: tabular-nums;
    }
</style>
