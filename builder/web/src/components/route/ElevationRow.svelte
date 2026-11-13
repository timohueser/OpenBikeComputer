<script lang="ts">
    import { gradeBands, gradePaths, profileGrades } from '../../lib/planner/grade-data';
    import { cumulative } from '../../lib/planner/geo';
    import { heightRange, profileHeightAt, profilePaths, profileSamples, sampleIndex, type ProfileLine } from '../../lib/planner/profile-data';
    import { formatHeight } from '../../lib/settings/format';
    import { appearance } from '../../lib/theme.svelte';

    /** The heights of a stretch of a line as the planner profile draws them: a plain area, and the line in the colour of
     * the steepness band of each segment, with the lowest and the highest height of the scale inside the box. Each climb
     * fills its stretch under the line, with a rule on the floor and its text over the top. Shares are 0 to 1 of the line. */
    let { line, from = 0, to = 1, climbs = [], lit, part = null, lines = [] }: {
        line: ProfileLine;
        from?: number;
        to?: number;
        /** Stretches with a long and a short text, such as "3.0 km · 7 %" and "7 %". */
        climbs?: { from: number; to: number; text: string; short: string }[];
        /** The index of the climb that shows in `--ink`. */
        lit?: number;
        /** The selected part: the texts of the climbs outside it are pale and whole, over the pale layer of `Lanes`. */
        part?: [number, number] | null;
        /** Dashed vertical lines, such as the ends of the days. */
        lines?: number[];
    } = $props();

    /** HEAD units over the 105 of `profilePaths` keep room for the texts. A text is about CHAR_PX wide for each character
     * and keeps TEXT_GAP_PX from the next one; a scale label is about SCALE_PX wide. */
    const HEAD = 20, CHAR_PX = 6.2, TEXT_GAP_PX = 10, SCALE_PX = 56;
    const uid = $props.id();
    let width = $state(0), height = $state(0);
    const samples = $derived(profileSamples(line));
    const grades = $derived(profileGrades(samples, cumulative(line.coordinates).at(-1) ?? 0));
    const span = $derived(Math.max(1e-6, to - from));
    // The scale fits the shown stretch, with the samples just outside it.
    const { low, high } = $derived(heightRange(samples.slice(Math.max(0, sampleIndex(samples, from) - 1), sampleIndex(samples, to) + 1)));
    const area = $derived(profilePaths(samples, low, high).area);
    const bands = $derived(gradePaths(samples, grades, low, high));
    const head = $derived(climbs.length ? HEAD : 0);
    const left = (share: number) => `${(share - from) / span * 100}%`;
    /** The CSS pixel row of a height in the box. */
    const yOf = (metres: number) => (head + 105 - (metres - low) / (high - low) * 100) / (105 + head) * height;

    /** A scale label stands at the left end of the box, or at the right end where the line would cross it at the left. */
    function scaleSide(top: number, floor: boolean) {
        const crosses = (a: number, b: number) => samples.slice(sampleIndex(samples, a), sampleIndex(samples, b) + 1)
            .some(({ height: metres }) => metres !== null && (floor ? yOf(metres) > top - 3 : yOf(metres) < top + 17));
        const reach = SCALE_PX / Math.max(1, width) * span;
        return crosses(from, from + reach) && !crosses(to - reach, to) ? 'right' : 'left';
    }
    const highTop = $derived(Math.max(0, yOf(high) - 7));
    const highSide = $derived(width ? scaleSide(highTop, false) : 'left');
    const lowSide = $derived(width ? scaleSide(height - 16, true) : 'left');

    /** The climbs in the shown stretch, each with the text that fits, the long or the short one or none. A text ends over
     * the top of its climb, over the rising line, and moves off a dashed line. */
    const shown = $derived.by(() => {
        const dashes = lines.filter(share => share > from && share <= to).map(share => (share - from) / span * width);
        let end = -Infinity;
        return climbs.map((climb, index) => ({ ...climb, index })).filter(climb => climb.to > from && climb.from < to).map(climb => {
            const a = Math.max(from, climb.from), b = Math.min(to, climb.to), top = (b - from) / span * width;
            const fits = (size: number) => {
                let start = Math.max(0, Math.min(width - size, top - size));
                const dash = dashes.find(d => d > start - 4 && d < start + size + 4);
                if (dash !== undefined) start = dash - start < size / 2 ? dash + 5 : dash - 5 - size;
                return start >= end + TEXT_GAP_PX ? start : null;
            };
            let text = '', start = 0;
            for (const candidate of [climb.text, climb.short]) {
                const at = fits(candidate.length * CHAR_PX);
                if (at !== null) { [text, start, end] = [candidate, at, at + candidate.length * CHAR_PX]; break; }
            }
            const peak = profileHeightAt(climb.to, samples);
            const inside = !part || (climb.from >= part[0] - 1e-9 && climb.to <= part[1] + 1e-9);
            return { a, b, index: climb.index, text, start, inside, y: peak === null ? 0 : yOf(peak) };
        });
    });
</script>

<div class="elevation" bind:clientWidth={width} bind:clientHeight={height}>
    <svg viewBox={`${from * 1000} ${-head} ${span * 1000} ${105 + head}`} preserveAspectRatio="none" aria-hidden="true">
        <defs>
            {#each shown as climb (climb.index)}
                <clipPath id={`${uid}-climb-${climb.index}`}><rect x={climb.a * 1000} y={-head} width={(climb.b - climb.a) * 1000} height={105 + head} /></clipPath>
            {/each}
        </defs>
        <path class="area" d={area} />
        {#each shown as climb (climb.index)}
            <path class="climb" class:lit={lit === climb.index} d={area} clip-path={`url(#${uid}-climb-${climb.index})`} />
        {/each}
        {#each bands as path, band (band)}
            {#if path}<path class="line" d={path} style:stroke={appearance.dark ? gradeBands[band].dark : gradeBands[band].color} />{/if}
        {/each}
    </svg>
    <span class="scale" class:right={highSide === 'right'} style:top={`${highTop}px`}>{formatHeight(high)}</span>
    <span class="scale" class:right={lowSide === 'right'} style:bottom="2px">{formatHeight(low)}</span>
    {#each lines as share, i (i)}
        {#if share > from && share <= to}<i style:left={left(share)}></i>{/if}
    {/each}
    {#each shown as climb (climb.index)}
        <b class="floor" class:lit={lit === climb.index} style:left={left(climb.a)} style:width={`${(climb.b - climb.a) / span * 100}%`}></b>
        {#if climb.text}
            <span class="text" class:lit={lit === climb.index} class:faint={!climb.inside} style:left={`${climb.start}px`} style:top={`${Math.max(0, climb.y - 17)}px`}>{climb.text}</span>
        {/if}
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
        fill: color-mix(in srgb, var(--ink-faint) 10%, var(--panel));
    }
    .climb {
        fill: color-mix(in srgb, var(--ink-faint) 24%, var(--panel));
    }
    .climb.lit {
        fill: color-mix(in srgb, var(--ink-faint) 38%, var(--panel));
    }
    .line {
        fill: none;
        stroke-width: 2.5;
        stroke-linejoin: round;
        vector-effect: non-scaling-stroke;
    }
    .scale, .text {
        position: absolute;
        font-size: 11px;
        line-height: 14px;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
        pointer-events: none;
    }
    .scale {
        left: 4px;
        color: var(--ink-faint);
    }
    .scale.right {
        right: 4px;
        left: auto;
    }
    i {
        position: absolute;
        top: 0;
        bottom: 0;
        border-left: 1px dashed var(--ink-soft);
    }
    .floor {
        position: absolute;
        bottom: 0;
        height: 2px;
        background: var(--ink-faint);
    }
    .floor.lit {
        background: var(--ink);
    }
    /* Over the pale layer of `Lanes`, so that a text is never cut by the edge of a part. */
    .text {
        z-index: 3;
        color: var(--ink-soft);
        font-weight: 500;
    }
    /* As pale as the rows under the pale layer of `Lanes`: the panel at its share `--pale` over `--ink-soft`. */
    .text.faint {
        color: color-mix(in srgb, var(--panel) calc(var(--pale) * 100%), var(--ink-soft));
    }
    .text.lit {
        color: var(--ink);
        font-weight: 600;
    }
</style>
