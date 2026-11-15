<!-- The height profile of a summary line as a static silhouette. A point with no height leaves a gap. -->
<script lang="ts">
    import type { Summary } from '../../lib/library/records';

    let { summary }: { summary: Pick<Summary, 'line' | 'heights'> } = $props();

    const WIDTH = 200, HEIGHT = 50;

    const paths = $derived.by(() => {
        const { line, heights } = summary;
        // The distance along the line on a flat projection: the summary keeps few points on a straight stretch.
        const along = [0];
        for (let i = 1; i < heights.length; i++) {
            const k = Math.cos(line[2 * i + 1] * Math.PI / 180);
            along.push(along[i - 1] + Math.hypot((line[2 * i] - line[2 * i - 2]) * k, line[2 * i + 1] - line[2 * i - 1]));
        }
        const known = heights.filter((h): h is number => h !== null);
        if (!known.length || !along.at(-1)) return [];
        const low = Math.min(...known), span = Math.max(...known) - low || 1;
        const x = (i: number) => (along[i] / along.at(-1)! * WIDTH).toFixed(1), y = (h: number) => (HEIGHT - 4 - (h - low) / span * (HEIGHT - 8)).toFixed(1);
        const runs: number[][] = [];
        heights.forEach((h, i) => {
            if (h === null) return;
            if (i && heights[i - 1] !== null) runs.at(-1)!.push(i);
            else runs.push([i]);
        });
        return runs.map(run => ({
            area: `M${x(run[0])},${HEIGHT} ${run.map(i => `L${x(i)},${y(heights[i]!)}`).join(' ')} L${x(run.at(-1)!)},${HEIGHT}Z`,
            top: run.map((i, n) => `${n ? 'L' : 'M'}${x(i)},${y(heights[i]!)}`).join(' '),
        }));
    });
</script>

<svg viewBox="0 0 {WIDTH} {HEIGHT}" preserveAspectRatio="none" aria-hidden="true">
    {#each paths as path, i (i)}
        <path class="area" d={path.area} />
        <path class="top" d={path.top} />
    {/each}
</svg>

<style>
    svg {
        display: block;
        width: 100%;
        height: 100%;
    }
    .area {
        fill: var(--parchment-2);
    }
    .top {
        fill: none;
        stroke: var(--ink-soft);
        stroke-width: 1.5;
        vector-effect: non-scaling-stroke;
    }
</style>
