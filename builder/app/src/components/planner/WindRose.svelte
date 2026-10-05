<script lang="ts">
    import { dayColor } from '../../lib/planner/day-colors';
    import { headPart } from '../../lib/planner/layers/climate';
    import { roseColors, type RoseView } from '../../lib/planner/layers/wind';
    import type { Theme } from '../../lib/planner/layers/data-layer';

    /** The daytime wind rose of a month and its facts: petals point where the wind blows; the route's travel direction crosses it. */
    let { shares, bearing, month, facts, theme }: RoseView & { theme: Theme } = $props();

    const RING = 44, LETTERS = RING + 10;
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
            return { d: `M0 0L${polar(r, a - 10.5)}A${r.toFixed(2)} ${r.toFixed(2)} 0 0 1 ${polar(r, a + 10.5)}Z`, fill };
        });
    });
    const travel = $derived(dayColor(1, theme));
    const arrow = `M0 ${RING - 6}V${-RING + 8}`;
</script>

<figure class="rose" class:route={bearing !== undefined}>
    <svg viewBox="-60 -60 120 120" role="img" aria-label={`Wind rose for ${month} daytime hours${bearing === undefined ? '' : ', with your direction of travel'}`}>
        <circle r={RING} class="ring" />
        {#each ['N', 'E', 'S', 'W'] as letter, i (letter)}
            <text x={i === 1 ? LETTERS : i === 3 ? -LETTERS : 0} y={i === 0 ? -LETTERS : i === 2 ? LETTERS : 0} class:north={i === 0}>{letter}</text>
        {/each}
        {#each petals as petal, s (s)}<path d={petal.d} fill={petal.fill} class="petal" />{/each}
        {#if bearing !== undefined}
            <g transform={`rotate(${bearing.toFixed(1)})`} stroke-linecap="round" stroke-linejoin="round">
                <path d={arrow} class="halo" stroke-width="6" />
                <path d={`M0 ${-RING - 3}l-6.5 11h13Z`} class="halo" stroke-width="3.5" />
                <path d={arrow} stroke={travel} stroke-width="2.75" />
                <path d={`M0 ${-RING - 3}l-6.5 11h13Z`} fill={travel} />
            </g>
        {/if}
    </svg>
    <ul>{#each facts as fact, f (f)}<li>{fact}</li>{/each}</ul>
    <p class="key">
        <span>Petals point where the wind blows</span>
        {#if bearing !== undefined}
            <span><i style:--key={colors.head}></i>Headwind</span>
            <span><i style:--key={colors.tail}></i>Tailwind</span>
            <span><i style:--key={colors.cross}></i>Crosswind</span>
            <span><i class="travel" style:--key={travel}></i>Your direction</span>
        {/if}
    </p>
</figure>

<style>
    /* Off the route the facts sit beside the rose; on it the key does, and the facts take the full width below. */
    .rose { display: grid; grid-template: "rose facts" auto "rose key" auto / 128px 1fr; align-content: center; gap: 0 14px; margin: 4px 0 0; }
    .route { grid-template: "rose key" auto "facts facts" auto / 128px 1fr; align-items: center; }
    svg { grid-area: rose; width: 128px; height: 128px; align-self: center; }
    .ring { fill: none; stroke: var(--line-strong); stroke-width: 1; }
    text { font-size: 9.5px; font-weight: 500; text-anchor: middle; dominant-baseline: central; fill: var(--ink-soft); }
    .north { font-weight: 700; fill: var(--ink); }
    .petal { stroke: var(--panel); stroke-width: 0.75; }
    .halo { fill: var(--panel); stroke: var(--panel); }
    ul { grid-area: facts; display: grid; gap: 6px; margin: 0; padding: 0; list-style: none; font-size: 13px; line-height: 1.35; color: var(--ink); align-self: end; }
    .route ul { margin-top: 10px; }
    .key { grid-area: key; display: grid; gap: 4px; margin: 10px 0 0; font-size: 12px; line-height: 1.3; color: var(--ink-soft); align-self: start; }
    .route .key { margin: 0; align-self: center; }
    .key span { display: flex; align-items: center; gap: 6px; }
    i { flex: none; width: 10px; height: 10px; border-radius: 2px; background: var(--key); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .travel { width: 14px; height: 2.75px; box-shadow: none; }
</style>
