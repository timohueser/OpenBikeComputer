<!-- The panel of "Where": the elevation profile of the route with its days. A drag across the profile sets a stretch, its
     two handles move the ends, and a tap or a day button sets that day. Under it are the part of the stretch or the day,
     and the areas by the place of the rider or near the point that the map selects. The places of "Find" along the route are dots on the profile and its facts
     are bars at its foot; the ones in the stretch are marked. -->
<script lang="ts">
    import { dayColor } from '../../lib/planner/day-colors';
    import { heightRange, profileHeightAt, profilePaths, profileSamples } from '../../lib/planner/profile-data';
    import { baseOf, NEAREST, partOf, sameValue, stretchWhere, whereWords } from '../../lib/planner/search/slots';
    import type { Quantity, SearchAnswer, SearchContext, Where } from '../../lib/planner/search/types';
    import { distanceIn, formatAlong, formatDistance } from '../../lib/settings/format';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { appearance } from '../../lib/theme.svelte';

    let { where, radius, plan, found, selection, onchange }: {
        where: Where; radius?: Quantity; plan: SearchContext['plan'];
        /** The point that the map selects. */
        selection?: Where;
        /** The places or the stretches of "Find" along the whole route. */
        found?: SearchAnswer;
        /** A new area or a new radius; `moving` while a drag or a handle changes the area. */
        onchange: (change: { where: Where | undefined } | { radius: Quantity | undefined }, moving?: boolean) => void;
    } = $props();

    /** A drag starts after DRAG_PX; a handle moves by STEP km and stops a STEP before the other one. Each day button shows
     * only its number when there are more than WORDS_DAYS days. */
    const DRAG_PX = 4, STEP = .1, WORDS_DAYS = 5;
    const total = $derived(plan.km.at(-1) ?? 0);
    const days = $derived(plan.days.filter(day => !day.rest));
    const base = $derived(baseOf(where, plan));
    const stretch = $derived(base && partOf(base, where.part));
    const [from, to] = $derived(stretch ?? [0, total]);
    const samples = $derived(profileSamples(plan.line));
    const { low, high } = $derived(heightRange(samples));
    const paths = $derived(profilePaths(samples, low, high));
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    /** Such as "km 15.7–32.4 · 16.7 km". */
    const readout = $derived(stretch ? `${formatAlong(from * 1000)}–${distanceIn(to * 1000).toFixed(1)} · ${formatDistance((to - from) * 1000)}` : whereWords(where)[0]);
    const share = (km: number) => `${km / total * 100}%`;
    /** The top of a dot on the profile line, in the 112 units of the plot height. */
    const top = (km: number) => `${(105 - ((profileHeightAt(km / total, samples) ?? low) - low) / (high - low) * 100) / 1.12}%`;
    /** With no area on the route, every dot and bar is marked. */
    const inside = (a: number, b = a) => !stretch || (b >= from && a <= to);

    let plot = $state<HTMLElement>();
    let down: { x: number; km: number; moved: boolean } | undefined;
    function kmAt(event: PointerEvent) {
        const box = plot!.getBoundingClientRect();
        return Math.min(total, Math.max(0, (event.clientX - box.left) / box.width * total));
    }
    function press(event: PointerEvent) {
        if (event.button) return;
        plot!.setPointerCapture(event.pointerId);
        down = { x: event.clientX, km: kmAt(event), moved: false };
    }
    function drag(event: PointerEvent) {
        if (!down || (!down.moved && Math.abs(event.clientX - down.x) < DRAG_PX)) return;
        down.moved = true;
        const km = kmAt(event);
        onchange({ where: stretchWhere([Math.min(down.km, km), Math.max(down.km, km)]) }, true);
    }
    function release() {
        const tap = down && !down.moved && days.length > 1 && (days.find(day => down!.km <= day.to) ?? days.at(-1));
        down = undefined;
        if (tap) setDay(tap.number);
    }

    /** A day keeps the part of the area; a second press on the day removes the day. */
    function setDay(day: number) {
        const kept = where.part ? { part: where.part } : undefined;
        onchange({ where: where.day !== day ? { day, ...kept } : kept && { scope: 'route', ...kept } });
    }
    function end(event: Event & { currentTarget: HTMLInputElement }, last: boolean) {
        const input = event.currentTarget, km = last ? Math.max(input.valueAsNumber, from + STEP) : Math.min(input.valueAsNumber, to - STEP);
        input.valueAsNumber = km;
        onchange({ where: stretchWhere(last ? [from, km] : [km, to]) }, true);
    }
    /** A part of the area on the route, such as of today, else of the whole route. */
    function setPart(part: string) {
        const { part: _, ...rest } = where, onRoute = base || where.day !== undefined || where.along;
        if (part !== (where.part ?? 'any')) onchange({ where: part === 'any' ? rest : onRoute ? { ...rest, part } : { scope: 'route', part } });
    }
    const around = $derived(where.scope === 'route' ? 'route' : where.scope === 'here' ? 'here' : sameValue(where, NEAREST) ? 'nearest' : where.anchor ? 'anchor' : '');
    const areas = $derived([
        ...total ? [{ value: 'route', label: 'Whole route' }] : [], { value: 'here', label: 'Near me' }, ...total ? [{ value: 'nearest', label: 'Nearest to me' }] : [],
        ...where.anchor || selection ? [{ value: 'anchor', label: 'Selected point' }] : [],
    ]);
    function setAround(next: string) {
        if (next === around) return;
        onchange({ where: next === 'route' ? { scope: 'route', ...where.part ? { part: where.part } : {} } : next === 'here' ? { scope: 'here' } : next === 'nearest' ? NEAREST : selection ?? where });
    }
</script>

<div class="where">
    {#if total}
        <p class="hint"><span>{days.length > 1 ? 'Drag on the profile, or tap a day' : 'Drag on the profile'}</span><em>{readout}</em></p>
        <!-- The handles under the profile are the keys of the drag. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="plot" bind:this={plot} onpointerdown={press} onpointermove={drag} onpointerup={release} onpointercancel={() => down = undefined}>
            <svg viewBox="0 0 1000 112" preserveAspectRatio="none" aria-hidden="true">
                <path class="area" d={paths.area} /><path class="line" d={paths.line} />
                {#each days as day, i (day.number)}
                    <path class="day" d={`M${day.from / total * 1000 + 3} 109H${day.to / total * 1000 - 3}`} style:stroke={dayColor(i + 1, theme)} />
                {/each}
            </svg>
            {#if stretch && (from > 0 || to < total)}<div class="stretch" style:left={share(from)} style:width={share(to - from)}></div>{/if}
            {#each found?.stretches ?? [] as fact, i (i)}
                <i class="fact" class:in={inside(fact.from, fact.to)} style:left={share(fact.from)} style:width={share(fact.to - fact.from)}></i>
            {/each}
            {#each found?.results ?? [] as place, i (i)}
                {#if place.position}<i class="dot" class:in={inside(place.position.along)} style:left={share(place.position.along)} style:top={top(place.position.along)}></i>{/if}
            {/each}
        </div>
        <div class="ends">
            <input type="range" min="0" max={total} step={STEP} value={from} aria-label="Start of the stretch" aria-valuetext={formatAlong(from * 1000)} oninput={event => end(event, false)} />
            <input type="range" min="0" max={total} step={STEP} value={to} aria-label="End of the stretch" aria-valuetext={formatAlong(to * 1000)} oninput={event => end(event, true)} />
        </div>
        {#if days.length > 1}
            <div class="days">
                {#each days as day (day.number)}
                    <button type="button" style:flex-grow={day.to - day.from} aria-pressed={where.day === day.number} aria-label={days.length > WORDS_DAYS ? `Day ${day.number}` : undefined}
                        onclick={() => setDay(day.number)}>{days.length > WORDS_DAYS ? day.number : `Day ${day.number}`}</button>
                {/each}
            </div>
        {/if}
        <h3>Within it</h3>
        <Segmented label="Part" value={where.part ?? 'any'} onChange={setPart}
            options={[{ value: 'any', label: 'Anywhere' }, { value: 'start', label: 'Start' }, { value: 'middle', label: 'Middle' }, { value: 'end', label: 'End' }]} />
        <h3>Or by where you are</h3>
    {:else}
        <p class="hint">With a route, the panel shows its profile and days.</p>
    {/if}
    <Segmented label="Area" value={around} onChange={setAround} wrap={areas.length > 3} options={areas} />
    {#if radius}
        <label class="radius">Within, in km<input type="number" min="0.1" max="100" step="0.1" value={radius.value}
            onchange={event => { const value = event.currentTarget.valueAsNumber; onchange({ radius: value > 0 ? { value, unit: 'km' } : undefined }); }} /></label>
    {/if}
</div>

<style>
    .where {
        display: flex;
        flex-direction: column;
        box-sizing: border-box;
        width: 420px;
        max-width: 100%;
        padding: 12px;
        font-size: 13px;
    }
    .hint {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 2px 12px;
        margin: 0 2px 6px;
        color: var(--ink-soft);
        font-size: 12.5px;
    }
    .hint span, .hint em { white-space: nowrap; }
    .hint em {
        margin-left: auto;
        color: var(--ink);
        font-style: normal;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
        text-align: right;
    }
    /* A vertical swipe scrolls the panel; a horizontal drag sets the stretch. */
    .plot {
        position: relative;
        height: 96px;
        margin: 0 9px;
        cursor: crosshair;
        touch-action: pan-y;
        user-select: none;
    }
    svg {
        display: block;
        width: 100%;
        height: 100%;
    }
    .area { fill: var(--parchment-3); }
    .line, .day {
        fill: none;
        stroke: var(--ink-soft);
        stroke-width: 1.4;
        vector-effect: non-scaling-stroke;
    }
    .day { stroke-width: 3; }
    .stretch {
        position: absolute;
        top: 0;
        bottom: 0;
        border-inline: 2px solid var(--rust);
        background: color-mix(in srgb, var(--amber) 24%, transparent);
        pointer-events: none;
    }
    .dot, .fact {
        position: absolute;
        background: var(--ink-faint);
        opacity: .5;
        pointer-events: none;
        transition: opacity 160ms, background-color 160ms;
    }
    .dot {
        width: 9px;
        height: 9px;
        margin: -4.5px 0 0 -4.5px;
        border-radius: 50%;
    }
    .dot.in {
        background: var(--rust);
        opacity: 1;
        box-shadow: 0 0 0 2px var(--panel);
    }
    .fact {
        bottom: 9px;
        height: 5px;
        border-radius: 3px;
    }
    .fact.in {
        background: var(--ink);
        opacity: .85;
    }
    /* Two range inputs on one track: only their thumbs take the pointer. A thumb is 18 px wide, so its centre meets the
       plot, which is 9 px in from each side. */
    .ends {
        position: relative;
        height: 24px;
        margin-top: -12px;
    }
    .ends input {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 24px;
        margin: 0;
        background: none;
        pointer-events: none;
        appearance: none;
    }
    .ends input::-webkit-slider-runnable-track { height: 6px; background: transparent; }
    .ends input::-moz-range-track { height: 6px; background: transparent; }
    .ends input::-webkit-slider-thumb {
        width: 18px;
        height: 18px;
        margin-top: -6px;
        border: 3px solid var(--panel);
        border-radius: 50%;
        background: var(--rust);
        box-shadow: 0 1px 3px color-mix(in srgb, var(--ink) 40%, transparent);
        pointer-events: auto;
        cursor: ew-resize;
        appearance: none;
    }
    .ends input::-moz-range-thumb {
        width: 12px;
        height: 12px;
        border: 3px solid var(--panel);
        border-radius: 50%;
        background: var(--rust);
        box-shadow: 0 1px 3px color-mix(in srgb, var(--ink) 40%, transparent);
        pointer-events: auto;
        cursor: ew-resize;
    }
    .ends input:focus-visible { outline: none; }
    .ends input:focus-visible::-webkit-slider-thumb { outline: 2px solid var(--ink); outline-offset: 2px; }
    .ends input:focus-visible::-moz-range-thumb { outline: 2px solid var(--ink); outline-offset: 2px; }
    .days {
        display: flex;
        gap: 2px;
        margin: 4px 9px 0;
    }
    .days button {
        flex: 1 1 0;
        min-width: 0;
        min-height: 30px;
        overflow: hidden;
        border: 0;
        border-radius: 5px;
        background: var(--parchment-2);
        color: var(--ink-soft);
        font: 600 12.5px var(--sans);
        white-space: nowrap;
        cursor: pointer;
        transition: background-color 120ms, color 120ms;
    }
    .days button:hover { color: var(--ink); }
    .days button[aria-pressed="true"] {
        background: var(--ink);
        color: var(--panel);
    }
    .days button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    h3 {
        margin: 14px 2px 6px;
        color: var(--ink-soft);
        font: 400 12px var(--sans);
    }
    .radius {
        display: flex;
        flex-direction: column;
        gap: 6px;
        margin-top: 12px;
        color: var(--ink-soft);
        font-size: 12px;
        font-weight: 500;
    }
    .radius input {
        box-sizing: border-box;
        width: 50%;
        min-height: 36px;
        padding: 6px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 400 13px var(--sans);
    }
    @media (prefers-reduced-motion: reduce) {
        .dot, .fact, .days button { transition: none; }
    }
</style>
