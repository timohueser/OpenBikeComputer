<!-- The panel of "Where": the elevation profile of the route with its days. A drag across the profile sets a stretch, its
     two handles move the ends, and a tap or a day button sets that day. Under it are the part of the stretch or the day,
     and the areas by the place of the rider or near the point that the map selects. The places of "Find" along the
     route are dots on the profile and its facts are bars at its foot; the ones in the stretch are marked, and the
     pointer names the one under it. -->
<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { cubicOut } from 'svelte/easing';
    import { Tween } from 'svelte/motion';
    import { dayColor } from '../../lib/planner/day-colors';
    import { heightRange, profileHeightAt, profilePaths, profileSamples } from '../../lib/planner/profile-data';
    import { baseOf, kindWord, NEAREST, partOf, sameValue, stretchWhere, type Stretch } from '../../lib/planner/search/slots';
    import type { Quantity, SearchAnswer, SearchContext, Where } from '../../lib/planner/search/types';
    import { distanceIn, formatAlong, formatDistance } from '../../lib/settings/format';
    import { still, unfold } from '../../lib/ui/search-bar/motion';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { appearance } from '../../lib/theme.svelte';

    let { where, radius, plan, found, selection, problem = '', onchange }: {
        where: Where; radius?: Quantity; plan: SearchContext['plan'];
        /** The point that the map selects. */
        selection?: Where;
        /** The places or the stretches of "Find" along the whole route. */
        found?: SearchAnswer;
        /** Why the search failed. The panel covers the list, which says it too. */
        problem?: string;
        /** A new area or a new radius; `moving` while a drag or a handle changes the area. */
        onchange: (change: { where: Where | undefined } | { radius: Quantity | undefined }, moving?: boolean) => void;
    } = $props();

    /** A drag starts after DRAG_PX; a handle moves by STEP km and stops a STEP before the other one. Each day button shows
     * only its number when there are more than WORDS_DAYS days. */
    const DRAG_PX = 4, STEP = .1, WORDS_DAYS = 5;
    const total = $derived(plan.km.at(-1) ?? 0);
    /** The first step of a handle at or after the end of the route, so the keys reach the end. */
    const max = $derived(Math.ceil(total / STEP - 1e-6) * STEP);
    const days = $derived(plan.days.filter(day => !day.rest));
    const base = $derived(baseOf(where, plan));
    const stretch = $derived(base && partOf(base, where.part));
    const [from, to] = $derived(stretch ?? [0, total]);
    const samples = $derived(profileSamples(plan.line));
    const { low, high } = $derived(heightRange(samples));
    const paths = $derived(profilePaths(samples, low, high));
    const theme = $derived(appearance.dark ? 'dark' : 'light');
    /** Such as "km 15.7–32.4 · 16.7 km". An area off the route has none: the slot names it. */
    const readout = $derived(stretch ? `${formatAlong(from * 1000)}–${distanceIn(to * 1000).toFixed(1)} · ${formatDistance((to - from) * 1000)}` : '');
    /** The ends of the stretch and of its base as the profile shows them: they glide to a new area, and follow a drag
     * or a handle at once. */
    const ends = () => [from, to, ...base ?? [0, total]];
    const shown = new Tween(untrack(ends), { easing: cubicOut });
    let moving = false;
    $effect.pre(() => { void shown.set(ends(), { duration: moving || still() ? 0 : 200 }); });
    const [lo, hi, baseLo, baseHi] = $derived(shown.current);
    const share = (km: number) => `${km / total * 100}%`;
    /** The top of a dot on the profile line, in the 112 units of the plot height. */
    const top = (km: number) => `${(105 - ((profileHeightAt(km / total, samples) ?? low) - low) / (high - low) * 100) / 1.12}%`;
    /** In the map view, every dot and bar is marked; in an area by a place off the route, none. */
    const inside = (a: number, b = a) => stretch ? b >= from && a <= to : !loose;
    /** The side of the profile where the name of a dot or a bar has no room to stand in its middle. */
    const side = (km: number) => km < total * .3 ? 'left' : km > total * .7 ? 'right' : '';

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
        move([Math.min(down.km, km), Math.max(down.km, km)]);
    }
    function move(next: Stretch) {
        moving = true;
        onchange({ where: stretchWhere(next) }, true);
        void tick().then(() => moving = false);
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
        move(last ? [from, km] : [km, to]);
    }
    /** A part of the area on the route, such as of today, else of the whole route. */
    function setPart(part: string) {
        const { part: _, ...rest } = where, onRoute = base || where.day !== undefined || where.along;
        if (part !== (where.part ?? 'any')) onchange({ where: part === 'any' ? rest : onRoute ? { ...rest, part } : { scope: 'route', part } });
    }
    const around = $derived(where.scope === 'route' ? 'route' : where.scope === 'here' ? 'here' : sameValue(where, NEAREST) ? 'nearest' : where.anchor ? 'anchor' : '');
    /** An area by a place off the route has no part. */
    const loose = $derived(!!where.near || ['here', 'nearest', 'anchor'].includes(around));
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
        <p class="hint"><em>{readout}</em><span>{days.length > 1 ? 'Drag on the profile, or tap a day' : 'Drag on the profile'}</span></p>
        <!-- The handles under the profile are the keys of the drag. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="plot" bind:this={plot} onpointerdown={press} onpointermove={drag} onpointerup={release} onpointercancel={() => down = undefined}>
            <svg viewBox="0 0 1000 112" preserveAspectRatio="none" aria-hidden="true">
                <path class="area" d={paths.area} /><path class="line" d={paths.line} />
                {#each days as day, i (day.number)}
                    <path class="day" d={`M${day.from / total * 1000 + 3} 109H${day.to / total * 1000 - 3}`} style:stroke={dayColor(i + 1, theme)} />
                {/each}
            </svg>
            <div class="shade" class:on={!!base && (base[0] > 0 || base[1] < total)} style:left={share(baseLo)} style:width={share(baseHi - baseLo)}></div>
            <div class="shade stretch" class:on={from > 0 || to < total} style:left={share(lo)} style:width={share(hi - lo)}></div>
            {#each found?.stretches ?? [] as fact, i (i)}
                <i class={['fact', side((fact.from + fact.to) / 2), inside(fact.from, fact.to) && 'in']} data-name={fact.label}
                    style:left={share(fact.from)} style:width={share(fact.to - fact.from)}></i>
            {/each}
            {#each found?.results ?? [] as place, i (i)}
                {#if place.position}
                    {@const km = place.position.along}
                    <i class={['dot', side(km), inside(km) && 'in']} data-name={`${place.name || kindWord(place.kind)} · ${formatAlong(km * 1000)}`}
                        style:left={share(km)} style:top={top(km)} style:animation-delay="{km / total * 160}ms"></i>
                {/if}
            {/each}
        </div>
        <!-- The handle that can move away is on top where the two meet. -->
        <div class="ends">
            <input type="range" min="0" {max} step={STEP} value={lo} style:z-index={from > total / 2 ? 1 : 0} aria-label="Start of the stretch" aria-valuetext={formatAlong(from * 1000)} oninput={event => end(event, false)} />
            <input type="range" min="0" {max} step={STEP} value={hi} aria-label="End of the stretch" aria-valuetext={formatAlong(to * 1000)} oninput={event => end(event, true)} />
        </div>
        {#if days.length > 1}
            <div class="days">
                {#each days as day (day.number)}
                    <button type="button" style:flex-grow={day.to - day.from} aria-pressed={where.day === day.number} aria-label={days.length > WORDS_DAYS ? `Day ${day.number}` : undefined}
                        onclick={() => setDay(day.number)}>{days.length > WORDS_DAYS ? day.number : `Day ${day.number}`}</button>
                {/each}
            </div>
        {/if}
        <fieldset disabled={loose}>
            <h3>Within it</h3>
            <Segmented label="Part" value={where.part ?? 'any'} onChange={setPart}
                options={[{ value: 'any', label: 'Anywhere' }, { value: 'start', label: 'Start' }, { value: 'middle', label: 'Middle' }, { value: 'end', label: 'End' }]} />
        </fieldset>
        <h3>Or by where you are</h3>
    {:else}
        <p class="note">With a route, the panel shows its profile and days.</p>
    {/if}
    <Segmented label="Area" value={around} onChange={setAround} columns={areas.length > 3 ? 2 : 0} options={areas} />
    {#if problem}<p class="note" in:unfold>{problem}</p>{/if}
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
    /* One line: where the two do not fit, the hint wraps out of sight and the reading stays. */
    .hint {
        display: flex;
        flex-flow: row-reverse wrap;
        justify-content: space-between;
        gap: 0 12px;
        height: 1.5em;
        margin: 0 2px 6px;
        overflow: hidden;
        color: var(--ink-soft);
        font-size: 12.5px;
        line-height: 1.5;
    }
    .hint span, .hint em { white-space: nowrap; }
    .hint em {
        color: var(--ink);
        font-style: normal;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
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
    /* The base of the area, and over it the stretch: where the area has no part, the two are one. */
    .shade {
        position: absolute;
        top: 0;
        bottom: 0;
        background: color-mix(in srgb, var(--amber) 10%, transparent);
        opacity: 0;
        pointer-events: none;
        transition: opacity 160ms;
    }
    .stretch {
        border-inline: 2px solid var(--rust);
        background: color-mix(in srgb, var(--amber) 16%, transparent);
    }
    .shade.on { opacity: 1; }
    .dot, .fact {
        position: absolute;
        background: color-mix(in srgb, var(--ink-faint) 50%, transparent);
        pointer-events: none;
        transition: background-color 160ms, box-shadow 160ms;
    }
    .dot {
        width: 9px;
        height: 9px;
        margin: -4.5px 0 0 -4.5px;
        border-radius: 50%;
        animation: arrive 200ms cubic-bezier(0.23, 1, 0.32, 1) backwards;
    }
    @keyframes arrive {
        from { opacity: 0; transform: scale(0.4); }
    }
    .dot.in {
        background: var(--rust);
        box-shadow: 0 0 0 2px var(--panel);
    }
    .fact {
        bottom: 9px;
        min-width: 4px;
        height: 5px;
        border-radius: 3px;
    }
    .fact.in { background: var(--ink); }
    /* A pointer that hovers names the dot or the bar under it; a press there still starts a drag. */
    @media (hover: hover) {
        .dot, .fact { pointer-events: auto; }
        .dot::before, .fact::before {
            content: "";
            position: absolute;
            inset: -6px;
        }
        .fact::before { inset-inline: 0; }
        .dot:hover, .fact:hover { z-index: 1; }
        .dot:hover { box-shadow: 0 0 0 2px var(--panel), 0 0 0 3.5px var(--ink); }
        .dot:hover::after, .fact:hover::after {
            content: attr(data-name);
            position: absolute;
            bottom: calc(100% + 7px);
            left: 50%;
            max-width: 240px;
            padding: 2px 7px;
            overflow: hidden;
            translate: -50%;
            border-radius: 6px;
            background: var(--ink);
            color: var(--panel);
            font: 600 12px/1.4 var(--sans);
            font-variant-numeric: tabular-nums;
            text-overflow: ellipsis;
            white-space: nowrap;
        }
        .left:hover::after { left: -6px; translate: none; }
        .right:hover::after { right: -6px; left: auto; translate: none; }
    }
    /* Two range inputs on one track: only their thumbs take the pointer. A thumb is 28 px wide for the finger and shows
       a disc of 18 px; its centre meets the plot, which is 14 px in from each side of the track. */
    .ends {
        position: relative;
        height: 28px;
        margin: -14px -5px 0;
        pointer-events: none;
    }
    .ends input {
        position: absolute;
        inset: 0;
        width: 100%;
        height: 28px;
        margin: 0;
        background: none;
        pointer-events: none;
        appearance: none;
    }
    .ends input::-webkit-slider-runnable-track { height: 6px; background: transparent; }
    .ends input::-moz-range-track { height: 6px; background: transparent; }
    .ends input::-webkit-slider-thumb {
        box-sizing: border-box;
        width: 28px;
        height: 28px;
        margin-top: -11px;
        border: 5px solid transparent;
        border-radius: 50%;
        background: var(--rust) padding-box;
        box-shadow: inset 0 0 0 3px var(--panel);
        filter: drop-shadow(0 1px 1.5px color-mix(in srgb, var(--ink) 40%, transparent));
        pointer-events: auto;
        cursor: ew-resize;
        appearance: none;
        transition: transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .ends input::-moz-range-thumb {
        box-sizing: border-box;
        width: 28px;
        height: 28px;
        border: 5px solid transparent;
        border-radius: 50%;
        background: var(--rust) padding-box;
        box-shadow: inset 0 0 0 3px var(--panel);
        filter: drop-shadow(0 1px 1.5px color-mix(in srgb, var(--ink) 40%, transparent));
        pointer-events: auto;
        cursor: ew-resize;
        transition: transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .ends input::-webkit-slider-thumb:hover, .ends input:active::-webkit-slider-thumb { transform: scale(1.12); }
    .ends input::-moz-range-thumb:hover, .ends input:active::-moz-range-thumb { transform: scale(1.12); }
    .ends input:focus-visible { outline: none; }
    .ends input:focus-visible::-webkit-slider-thumb { outline: 2px solid var(--ink); outline-offset: -1px; }
    .ends input:focus-visible::-moz-range-thumb { outline: 2px solid var(--ink); outline-offset: -1px; }
    .days {
        display: flex;
        gap: 2px;
        margin: 2px 9px 0;
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
        transition: background-color 120ms, color 120ms, transform 140ms cubic-bezier(0.23, 1, 0.32, 1);
    }
    .days button:hover { color: var(--ink); }
    .days button:active { transform: scale(0.97); }
    .days button[aria-pressed="true"] {
        background: var(--ink);
        color: var(--panel);
    }
    .days button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    fieldset {
        min-width: 0;
        margin: 0;
        padding: 0;
        border: 0;
        transition: opacity 160ms;
    }
    fieldset:disabled {
        opacity: .45;
        pointer-events: none;
    }
    h3 {
        margin: 14px 2px 6px;
        color: var(--ink-soft);
        font: 400 12px var(--sans);
    }
    .note {
        margin: 10px 2px 0;
        color: var(--ink-soft);
        line-height: 1.45;
    }
    .note:first-child { margin: 0 2px 10px; }
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
        .shade, .dot, .fact, .days button, fieldset { transition: none; }
        .dot { animation: none; }
        .ends input::-webkit-slider-thumb { transition: none; }
        .ends input::-moz-range-thumb { transition: none; }
    }
</style>
