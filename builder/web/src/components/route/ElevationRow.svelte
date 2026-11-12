<script lang="ts">
    import { gradeBand, gradeBands, profileGrades } from '../../lib/planner/grade-data';
    import { cumulative } from '../../lib/planner/geo';
    import { heightRange, profileSamples, sampleIndex, type ProfileLine } from '../../lib/planner/profile-data';
    import { formatHeight } from '../../lib/settings/format';
    import { appearance } from '../../lib/theme.svelte';

    /** The heights of a stretch of a line: the area under them in the colour of the steepness band of each segment, the
     * lowest and the highest height of the scale to the left of the box, and labels at the foot. Shares are 0 to 1 of the line. */
    let { line, from = 0, to = 1, labels = [], lines = [] }: {
        line: ProfileLine;
        from?: number;
        to?: number;
        /** A text at the foot of the middle of a stretch, such as the grade of a climb. */
        labels?: { from: number; to: number; text: string }[];
        /** Dashed vertical lines, such as the ends of the days. */
        lines?: number[];
    } = $props();

    /** A label shows only on a stretch at least this wide, in CSS pixels. */
    const LABEL_PX = 40;
    let width = $state(0);
    const samples = $derived(profileSamples(line));
    const grades = $derived(profileGrades(samples, cumulative(line.coordinates).at(-1) ?? 0));
    const span = $derived(Math.max(1e-6, to - from));
    // The scale fits the shown stretch, with the samples just outside it.
    const { low, high } = $derived(heightRange(samples.slice(Math.max(0, sampleIndex(samples, from) - 1), sampleIndex(samples, to) + 1)));
    // For each band, the outline of its segments and the area under them; consecutive segments of one band make one piece.
    const bands = $derived.by(() => {
        const paths = gradeBands.map(() => ({ line: '', area: '' }));
        const y = (height: number) => 100 - (height - low) / (high - low) * 100;
        let previous = -1, foot = '';
        for (let i = 0; i < grades.length; i++) {
            const a = samples[i], b = samples[i + 1];
            const band = a.height === null || b.height === null ? -1 : gradeBand(grades[i]);
            if (band !== previous && previous >= 0) paths[previous].area += `${foot}Z`;
            if (band < 0) { previous = -1; continue; }
            if (band !== previous) {
                paths[band].line += `M${a.progress * 1000} ${y(a.height!)}`;
                paths[band].area += `M${a.progress * 1000} 100L${a.progress * 1000} ${y(a.height!)}`;
            }
            const point = `L${b.progress * 1000} ${y(b.height!)}`;
            paths[band].line += point;
            paths[band].area += point;
            foot = `L${b.progress * 1000} 100`;
            previous = band;
        }
        if (previous >= 0) paths[previous].area += `${foot}Z`;
        return paths;
    });
    const shownLabels = $derived(labels.filter(label => label.to > from && label.from < to && (label.to - label.from) / span * width >= LABEL_PX));
    const left = (share: number) => `${(share - from) / span * 100}%`;
</script>

<div class="elevation" bind:clientWidth={width}>
    <span class="scale" style:top="0">{formatHeight(high)}</span>
    <span class="scale" style:bottom="0">{formatHeight(low)}</span>
    <svg viewBox={`${from * 1000} 0 ${span * 1000} 100`} preserveAspectRatio="none" aria-hidden="true">
        {#each bands as path, band (band)}
            {#if path.line}
                {@const colour = appearance.dark ? gradeBands[band].dark : gradeBands[band].color}
                <path class="area" d={path.area} style:fill={colour} />
                <path class="line" d={path.line} style:stroke={colour} />
            {/if}
        {/each}
    </svg>
    {#each lines as share (share)}
        {#if share > from && share < to}<i style:left={left(share)}></i>{/if}
    {/each}
    {#each shownLabels as label (label.from)}
        <span class="label" style:left={left((Math.max(from, label.from) + Math.min(to, label.to)) / 2)}>{label.text}</span>
    {/each}
</div>

<style>
    .elevation {
        position: relative;
        height: 100%;
    }
    svg {
        display: block;
        width: 100%;
        height: 100%;
        overflow: hidden;
    }
    .area {
        fill-opacity: .3;
    }
    .line {
        fill: none;
        stroke-width: 2;
        stroke-linejoin: round;
        vector-effect: non-scaling-stroke;
    }
    .scale {
        position: absolute;
        right: calc(100% + 6px);
        color: var(--ink-faint);
        font-size: 11px;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    i {
        position: absolute;
        top: 0;
        bottom: 0;
        border-left: 1px dashed var(--ink-soft);
    }
    .label {
        position: absolute;
        bottom: 4px;
        padding: 1px 6px;
        border-radius: 4px;
        background: var(--ink);
        color: var(--panel);
        font: 600 11px/1.4 var(--sans);
        white-space: nowrap;
        transform: translateX(-50%);
        pointer-events: none;
    }
</style>
