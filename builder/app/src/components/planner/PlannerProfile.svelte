<script lang="ts">
    import Segmented from './Segmented.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { profileHeightAt, profileHeights } from '../../lib/planner/profile-data';
    import type { Day } from '../../lib/planner/editor';

    let {
        total, days, dayLabels, theme = 'light', activeNight, band, focus = null, window: view = { from: 0, to: 1 }, height = 190,
        onNight, onDayEndDrag, onHover,
    }: {
        /** Route length in km. */
        total: number;
        days: Day[];
        /** Riding number → calendar number. */
        dayLabels: Record<number, number>;
        theme?: 'light' | 'dark';
        activeNight: number;
        focus?: { from: number; to: number; label: string } | null;
        /** The suggested overnight stretch, as route progress. */
        band: { from: number; to: number; blocked: boolean } | null;
        /** The stretch of the route the map shows, as route progress. */
        window?: { from: number; to: number };
        height?: number;
        onNight: (night: number) => void;
        onDayEndDrag: (night: number, progress: number) => void;
        onHover: (progress: number | null) => void;
    } = $props();

    // Heights map to y = 105 - (h - 350) / 5, so the grid lines at y 25, 65 and 105 are 750, 550 and 350 m.
    const line = profileHeights.map((h, i) => `${i / 120 * 1000},${105 - (h - 350) / 5}`).join(' ');
    let plot: HTMLDivElement;
    let hover = $state<number | null>(null);
    let drag = $state<{ night: number; progress: number; moved: boolean } | null>(null);
    let range = $state<'map' | 'route'>('map');

    const shown = $derived(range === 'map' ? focus ?? view : { from: 0, to: 1 });
    const origin = $derived(range === 'map' && focus ? focus.from : 0);
    const span = $derived(Math.max(1e-6, shown.to - shown.from));
    const ticks = $derived([0, .25, .5, .75, 1].map(t => (shown.from + t * span - origin) * total));
    // The day ends inside the shown stretch; with none inside, the nearest one on each side sits at the edge so it can still be dragged in.
    const handles = $derived.by(() => {
        const ends = days.slice(0, -1).map(day => ({ day, at: drag?.night === day.number ? drag.progress : day.to }));
        const inside = ends.filter(end => end.at >= shown.from && end.at <= shown.to).map(end => ({ ...end, x: x(end.at) }));
        if (inside.length) return inside;
        const before = ends.filter(end => end.at < shown.from).at(-1);
        const after = ends.find(end => end.at > shown.to);
        return [...(before ? [{ ...before, x: 0 }] : []), ...(after ? [{ ...after, x: 100 }] : [])];
    });

    function x(progress: number) {
        return (progress - shown.from) / span * 100;
    }

    function progressAt(event: PointerEvent) {
        const box = plot.getBoundingClientRect();
        return shown.from + Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)) * span;
    }

    function track(event: PointerEvent) {
        const progress = progressAt(event);
        if (drag) {
            const day = days[drag.night - 1];
            drag.progress = Math.max(day.from + .005, Math.min(days[drag.night].to - .005, progress));
            drag.moved = true;
        }
        hover = drag?.progress ?? progress;
        onHover(hover);
    }

    function leave() {
        if (drag) return;
        hover = null;
        onHover(null);
    }

    function press(event: PointerEvent, day: Day) {
        if (event.button !== 0) return;
        event.preventDefault();
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
        drag = day.pinned ? null : { night: day.number, progress: day.to, moved: false };
        if (day.pinned) onNight(day.number);
    }

    function release() {
        if (!drag) return;
        const { night, progress, moved } = drag;
        drag = null;
        if (moved) onDayEndDrag(night, progress);
        else onNight(night);
    }
</script>

<section class="elevation" style:height={`${height}px`} aria-label="Elevation profile">
    <div class="title">
        <strong>Elevation</strong>
        <Segmented compact label="Profile range" value={range} onChange={(value) => range = value}
            options={[{ value: 'map', label: focus?.label ?? 'Map view' }, { value: 'route', label: 'Whole route' }]} />
        <span class="example">Illustrative profile</span>
        <span class="distance">{(span * total).toFixed(1)} km</span>
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="plot" bind:this={plot} onpointermove={track} onpointerleave={leave} onpointerup={release} onpointercancel={() => drag = null}>
        <span class="height" style:top="22%">750 m</span>
        <span class="height" style:top="94%">350 m</span>
        <svg viewBox={`${shown.from * 1000} 0 ${span * 1000} 112`} preserveAspectRatio="none" role="img" aria-label={`${focus && range === 'map' ? focus.label : 'Route'} elevation profile. Distances start at ${origin > 0 ? 'the day start' : 'the route start'}. The shaded band is the suggested overnight stretch.`}>
            <path d="M0 25H1000M0 65H1000M0 105H1000" class="grid" />
            <polygon points={`0,112 ${line} 1000,112`} class="terrain" />
            {#if band && !band.blocked}<rect x={band.from * 1000} width={Math.max(0, band.to - band.from) * 1000} y="0" height="112" class="band" />{/if}
            {#each days as day (day.number)}
                <svg x={day.from * 1000} width={Math.max(0, day.to - day.from) * 1000} height="112" viewBox={`${day.from * 1000} 0 ${Math.max(0, day.to - day.from) * 1000} 112`} preserveAspectRatio="none" overflow="hidden">
                    <polyline points={line} style:stroke={dayColor(day.number, theme)} />
                </svg>
            {/each}
            {#each days.slice(0, -1) as day (day.number)}
                <line x1={day.to * 1000} x2={day.to * 1000} y1="0" y2="112" class:pinned={day.pinned} />
            {/each}
        </svg>
        {#if hover !== null}
            <span class="readout" style:left={`${x(hover)}%`}>
                <span class="chip" class:flip={x(hover) > 80}>{((hover - origin) * total).toFixed(1)} km{origin > 0 ? ` into ${focus!.label.toLowerCase()}` : ''} · {Math.round(profileHeightAt(hover))} m</span>
            </span>
        {/if}
        {#each handles as { day, x: left } (day.number)}
            <button
                type="button"
                class="handle"
                class:pinned={day.pinned}
                class:moved={day.split}
                class:active={activeNight === day.number}
                style:left={`${left}%`}
                style:--day-color={dayColor(day.number, theme)}
                title={day.pinned ? 'Pinned · Change overnight in the day' : `${day.split ? 'Day end you moved' : 'Day end suggested'} · drag along the route`}
                aria-label={`Day ${dayLabels[day.number] ?? day.number} ends here${day.pinned ? ', pinned' : day.split ? ', moved by you' : ', suggested'}. Show the day.`}
                onpointerdown={(event) => press(event, day)}
                onclick={(event) => { if (event.detail === 0) onNight(day.number); }}
            >{dayLabels[day.number] ?? day.number}</button>
        {/each}
    </div>
    <div class="axis">
        {#each ticks as km, i (i)}<span>{km.toFixed(span < .25 ? 1 : 0)} km</span>{/each}
    </div>
</section>

<style>
    .elevation {
        --terrain: var(--parchment-2);
        --band: color-mix(in srgb, var(--amber) 24%, var(--panel));
        container-type: inline-size;
        display: flex;
        flex-direction: column;
        flex: none;
        min-height: 130px;
        padding: 12px 24px 8px;
        background: var(--panel);
    }
    .title {
        display: flex;
        align-items: center;
        gap: 12px;
        margin-bottom: 8px;
        font-size: 13px;
    }
    .title strong {
        font: 600 14px var(--sans);
    }
    @container (max-width: 560px) { .example { display: none; } }
    .title span {
        color: var(--ink-faint);
    }
    .distance {
        margin-left: auto;
        font-variant-numeric: tabular-nums;
    }
    .plot {
        position: relative;
        flex: 1;
        min-height: 40px;
        margin: 0 12px 0 48px;
        touch-action: none;
    }
    .plot > svg {
        width: 100%;
        height: 100%;
        overflow: hidden;
    }
    .height {
        position: absolute;
        left: -48px;
        transform: translateY(-50%);
        font-size: 11px;
        color: var(--ink-faint);
        font-variant-numeric: tabular-nums;
    }
    .grid {
        fill: none;
        stroke: var(--line);
        stroke-width: 1;
        vector-effect: non-scaling-stroke;
    }
    .terrain {
        fill: var(--terrain);
    }
    .band {
        fill: var(--band);
    }
    polyline {
        fill: none;
        stroke-width: 2;
        vector-effect: non-scaling-stroke;
    }
    line {
        stroke: var(--ink-faint);
        stroke-width: 1;
        stroke-dasharray: 3 4;
        vector-effect: non-scaling-stroke;
    }
    line.pinned {
        stroke: var(--ink);
        stroke-dasharray: none;
    }
    .readout {
        position: absolute;
        top: 0;
        bottom: 0;
        width: 1px;
        background: var(--ink);
        pointer-events: none;
    }
    .chip {
        position: absolute;
        top: 0;
        left: 6px;
        padding: 2px 6px;
        border-radius: 6px;
        background: var(--ink);
        color: var(--panel);
        font: 600 11px var(--sans);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .chip.flip {
        left: auto;
        right: 6px;
    }
    .handle {
        position: absolute;
        bottom: 4px;
        width: 24px;
        height: 24px;
        display: grid;
        place-items: center;
        padding: 0;
        transform: translateX(-50%);
        border: 2px dashed var(--day-color);
        border-radius: 50%;
        background: var(--panel);
        color: var(--day-color);
        font: 700 11px var(--sans);
        cursor: ew-resize;
        touch-action: none;
    }
    .handle.moved {
        border-style: dotted;
    }
    .handle.moved::after {
        content: "";
        position: absolute;
        bottom: 2px;
        width: 3px;
        height: 3px;
        border-radius: 50%;
        background: currentColor;
    }
    .handle.pinned {
        border-style: solid;
        background: var(--day-color);
        color: var(--panel);
        cursor: pointer;
    }
    .handle.active {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .handle:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: 3px;
    }
    .axis {
        display: flex;
        justify-content: space-between;
        margin: 8px 0 0 48px;
        margin-right: 12px;
        font-size: 11px;
        color: var(--ink-faint);
        font-variant-numeric: tabular-nums;
    }
</style>
