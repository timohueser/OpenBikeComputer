<script lang="ts">
    import { dayColor } from '../../lib/planner/day-colors';
    import { headPart } from '../../lib/planner/layers/climate';
    import { roseColors, type RoseView } from '../../lib/planner/layers/wind';
    import type { Theme } from '../../lib/planner/layers/data-layer';

    /** The daytime wind rose of a month: petals point where the wind blows; the route's travel direction crosses it. */
    let { shares, bearing, month, theme }: RoseView & { theme: Theme } = $props();

    const RING = 40;
    const colors = $derived(roseColors(theme));
    const polar = (radius: number, degrees: number) => {
        const a = degrees * Math.PI / 180;
        return `${(radius * Math.sin(a)).toFixed(2)} ${(-radius * Math.cos(a)).toFixed(2)}`;
    };
    // Petal area follows the share, so the radius follows its square root.
    const petals = $derived.by(() => {
        const most = Math.max(...shares);
        return shares.map((share, s) => {
            const r = RING * Math.sqrt(share / most), a = 22.5 * s;
            const fill = bearing === undefined ? colors.even : headPart(s, bearing) >= 0.5 ? colors.head : headPart(s, bearing + 180) >= 0.5 ? colors.tail : colors.cross;
            return { d: `M0 0L${polar(r, a - 10)}A${r.toFixed(2)} ${r.toFixed(2)} 0 0 1 ${polar(r, a + 10)}Z`, fill };
        });
    });
    const travel = $derived(dayColor(1, theme));
</script>

<figure class="rose">
    <svg viewBox="-54 -54 108 108" role="img" aria-label={`Wind rose for ${month} daytime hours${bearing === undefined ? '' : ', with your direction of travel'}`}>
        <circle r={RING} class="ring" />
        <text y={-RING - 4} class="north">N</text>
        {#each petals as petal, s (s)}<path d={petal.d} fill={petal.fill} class="petal" />{/each}
        {#if bearing !== undefined}
            <g transform={`rotate(${bearing.toFixed(1)})`} stroke={travel} fill={travel}>
                <line y1={RING - 2} y2={-RING - 4} stroke-width="2.5" stroke-linecap="round" />
                <path d={`M0 ${-RING - 11}l-5.5 9h11Z`} stroke-width="1" stroke-linejoin="round" />
            </g>
        {/if}
    </svg>
    <figcaption>
        <strong>{month}, daytime</strong>
        <span>Petals point where the wind blows</span>
        {#if bearing !== undefined}
            <span><i class="key" style:--key={colors.head}></i>Headwind <i class="key" style:--key={colors.tail}></i>Tailwind</span>
            <span><i class="travel" style:--key={travel}></i>Your direction</span>
        {/if}
    </figcaption>
</figure>

<style>
    .rose { display: flex; align-items: center; gap: 12px; margin: 10px 0 0; }
    svg { flex: none; width: 96px; height: 96px; overflow: visible; }
    .ring { fill: none; stroke: var(--line-strong); stroke-width: 1; }
    .north { font-size: 9px; font-weight: 600; text-anchor: middle; fill: var(--ink-soft); }
    .petal { stroke: var(--panel); stroke-width: 0.75; }
    figcaption { display: grid; gap: 3px; font-size: 12px; line-height: 1.3; color: var(--ink-soft); }
    strong { font-weight: 600; color: var(--ink); }
    figcaption span { display: flex; align-items: center; gap: 5px; }
    .key { flex: none; width: 10px; height: 10px; border-radius: 2px; background: var(--key); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .key:not(:first-child) { margin-left: 6px; }
    .travel { flex: none; width: 14px; height: 2.5px; border-radius: 2px; background: var(--key); }
</style>
