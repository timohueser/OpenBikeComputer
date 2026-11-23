<script lang="ts">
    import { tick, untrack } from 'svelte';
    import { fade } from 'svelte/transition';
    import Icon from '../../lib/ui/Icon.svelte';
    import DayCard from './DayCard.svelte';
    import DayLine from './DayLine.svelte';
    import NightSeam from './NightSeam.svelte';
    import { orderedRoutePoints, type Day, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { heightRange, profilePaths, profileSamples, sampleSlice } from '../../lib/planner/profile-data';
    import { daysAround } from '../../lib/planner/stops';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { step } from '../../lib/planner/week';
    import { formatDistance, formatHeight } from '../../lib/settings/format';
    import { still } from '../../lib/ui/search-bar/motion';

    let {
        trip, line, days, theme, expandedDay, nights, conflicts, revealId, hoveredId = null, onHover,
        onToggle, onOverview, onInspect, onShowEnd, onShowConflict, week = null, onWeek,
    }: {
        trip: Trip;
        /** The line of the trip, once it is calculated. */
        line?: RoutingLine;
        days: Day[];
        theme: 'light' | 'dark';
        /** Number of the open day. */
        expandedDay: number | null;
        nights: NightChoices;
        conflicts: [RoutePoint, RoutePoint][];
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        /** The point whose row lights up for a moment; `night-2` lights night 2. */
        revealId: string | null;
        onToggle: (day: number) => void;
        onOverview: () => void;
        onInspect: (point: RoutePoint) => void;
        onShowEnd: (day: Day) => void;
        onShowConflict: (points: [RoutePoint, RoutePoint]) => void;
        /** The week in focus of a long trip, by its first and last day: the list shows only its days. */
        week?: { start: number; end: number } | null;
        /** Moves the week to start at this day. */
        onWeek?: (start: number) => void;
    } = $props();

    let root = $state<HTMLDivElement>();
    let back = $state<HTMLButtonElement>();
    let stepper = $state<HTMLDivElement>();
    const focused = $derived(days.find(day => day.number === expandedDay));
    const weekDays = $derived(week ? days.slice(week.start - 1, week.end) : days);
    // The night whose seam is open between the cards of its two days; the list then shows only these.
    const chosen = $derived(focused ? null : nights.choosing);
    const visible = $derived(chosen !== null ? days.slice(chosen - 1, chosen + 1) : weekDays);
    // Where the day ends in view on the seam; the two cards show the days with that end.
    let end = $state<{ km: number; town: string } | null>(null);
    const around = $derived(chosen !== null && end ? daysAround(trip, line, chosen, end.km) : null);
    const shown = (day: Day) => around?.[day.number - chosen!] ?? day;
    const ends = $derived(orderedRoutePoints(trip));
    /** The town of each night by the number of its day: of the day end in view, of the place that the rider set, else of
     * the suggestion. */
    const nightTowns = $derived(days.map(day => around && day.number === chosen ? end!.town
        : day.pinned ? day.pinned.locality ?? day.pinned.label : nights.around(day.number)?.stop.name ?? lastTown(day.number)));
    // A night keeps the town of its last suggestion until its stops are found again, so that a title does not read
    // "Night 1" for the moment in which the route is calculated.
    const towns = new Map<number, string>();
    function lastTown(night: number) {
        const state = nights.found(night)?.state;
        return state === 'ready' || state === 'error' ? undefined : towns.get(night);
    }
    $effect(() => { for (const day of days) { const town = nights.around(day.number)?.stop.name; if (town) towns.set(day.number, town); } });
    const fromOf = (day: Day) => day.number === 1 ? ends[0]?.label ?? '' : nightTowns[day.number - 2] ?? `Night ${day.number - 1}`;
    const toOf = (day: Day) => day.number === days.length ? (trip.loop ? ends[0] : ends.at(-1))?.label ?? '' : nightTowns[day.number - 1] ?? `Night ${day.number}`;
    const samples = $derived(profileSamples(line));
    const heights = $derived(heightRange(samples));
    const profileOf = (day: Day) => samples.length
        ? { ...profilePaths(sampleSlice(samples, day.from, day.to), heights.low, heights.high), view: `${day.from * 1000} 0 ${(day.to - day.from) * 1000} 105` } : undefined;

    // A number, so that a new week object with the same days does not scroll the list.
    const weekStart = $derived(week?.start);
    $effect(() => {
        void [expandedDay, weekStart];
        tick().then(() => {
            const scroll = root?.closest('.pane-scroll');
            if (scroll) scroll.scrollTop = 0;
        });
    });

    // The list takes its new layout at once when a night or a stop opens or closes, and moves only by transform and
    // opacity, which stay smooth while the map moves. The parts with `data-glide` that stay glide from where they stood
    // to their new places; a part that leaves fades where it stood. `hold` takes the places before the change. The panel
    // above the list changes with the night, so the page calls `hold` before any of it moves; the first call counts.
    let stood = new Map<Element, number>();
    const glides = new WeakMap<Element, Animation>();
    const parts = () => [...root?.querySelectorAll<HTMLElement>('[data-glide]') ?? []];
    export function hold() {
        if (stood.size) return;
        stood = new Map(parts().map(part => [part, part.getBoundingClientRect().top]));
        // After the list has its new layout, scroll position and focus, and before it shows.
        requestAnimationFrame(glide);
    }
    // A stop that opens or closes, and the stops that arrive, move the parts under them.
    $effect.pre(() => {
        void [nights.stopId, chosen !== null && nights.found(chosen)?.state];
        untrack(hold);
    });
    function glide() {
        const from = stood, top = root?.getBoundingClientRect().top ?? 0;
        stood = new Map();
        if (!from.size || still()) return;
        // A part that leaves is inert. Out of the flow, it lets the parts that stay close its gap.
        for (const part of parts()) {
            const was = from.get(part);
            Object.assign(part.style, part.inert && was !== undefined ? { position: 'absolute', top: `${was - top}px`, width: `${part.offsetWidth}px` } : { position: '', top: '', width: '' });
        }
        for (const part of parts()) {
            glides.get(part)?.cancel();
            const was = from.get(part), by = part.inert || was === undefined ? 0 : was - part.getBoundingClientRect().top;
            // A part that glides passes over the others.
            part.style.zIndex = by ? '4' : '';
            if (!by) continue;
            const move = part.animate([{ transform: `translateY(${by}px)` }, { transform: 'none' }], { duration: 240, easing: 'cubic-bezier(.23, 1, .32, 1)' });
            move.onfinish = () => part.style.zIndex = '';
            glides.set(part, move);
        }
    }
    const ms = (duration: number) => still() ? 0 : duration;

    // The day list keeps its scroll position under an open night; a night opened from an open day returns to the top of
    // the list, as leaving the day does. The night opens at the top. A closed night gives the focus back to the button
    // that opens it.
    let wasNight: number | null = null, wasDay: number | null = null, listTop = 0;
    $effect.pre(() => {
        const next = nights.choosing, day = expandedDay;
        untrack(() => {
            const was = wasNight, fromDay = wasDay !== null;
            wasNight = next;
            wasDay = day;
            if (was === next) return;
            const scroll = root?.closest('.pane-scroll');
            if (was === null) listTop = fromDay ? 0 : scroll?.scrollTop ?? 0;
            tick().then(() => {
                if (next !== null) {
                    if (scroll) scroll.scrollTop = 0;
                    back?.focus({ preventScroll: true });
                } else if (!focused) {
                    if (scroll) scroll.scrollTop = listTop;
                    root?.querySelector<HTMLElement>(`[data-night="${was}"]`)?.focus();
                }
            });
        });
    });

    /** Opens a day. The focus goes to the arrow of the next day, or of the day before on the last day, when the control
     * that had it is gone or disabled. */
    async function openDay(day: number) {
        onToggle(day);
        await tick();
        const arrows = [...stepper?.querySelectorAll('button') ?? []].filter(arrow => !arrow.disabled);
        if (!arrows.some(arrow => arrow === document.activeElement)) arrows.at(-1)?.focus({ preventScroll: true });
    }

    async function overview() {
        const day = expandedDay;
        onOverview();
        await tick();
        root?.querySelector<HTMLElement>(`[data-day="${day}"]`)?.focus();
    }

    function conflictOf(day: number) {
        return conflicts.find(([, after]) => after.night === day) ?? null;
    }
</script>

{#if week && chosen === null}
    <nav class="week" aria-label="Week">
        <button type="button" class="icon" aria-label="7 days back" disabled={week.start === 1} onclick={() => onWeek?.(step(days.length, week.start, -7))}><Icon path="m15 5-7 7 7 7" size={16} /></button>
        <strong>Days {week.start}–{week.end}</strong>
        <span>{formatDistance(weekDays.reduce((sum, day) => sum + day.distance, 0) * 1000)}{line?.unknownElevationKm === 0 ? ` · ↑ ${formatHeight(weekDays.reduce((sum, day) => sum + day.ascent, 0))}` : ''}</span>
        <button type="button" class="icon" aria-label="7 days on" disabled={week.end === days.length} onclick={() => onWeek?.(step(days.length, week.start, 7))}><Icon name="chevron" size={16} /></button>
    </nav>
{/if}
{#if focused}
    <nav class="day-nav" aria-label="Day navigation" style:top={week ? '36px' : undefined}>
        <button type="button" class="overview" onclick={overview}><Icon name="back" size={16} />All days</button>
        <div class="stepper" bind:this={stepper}>
            <button type="button" class="icon" aria-label="Previous riding day" disabled={focused.number === 1} onclick={() => openDay(focused.number - 1)}><Icon path="m15 5-7 7 7 7" size={16} /></button>
            <strong>Day {focused.number} of {days.length}</strong>
            <button type="button" class="icon" aria-label="Next riding day" disabled={focused.number === days.length} onclick={() => openDay(focused.number + 1)}><Icon name="chevron" size={16} /></button>
        </div>
    </nav>
{:else if chosen !== null}
    <nav class="day-nav" aria-label="Night navigation">
        <button type="button" class="overview" bind:this={back} onclick={() => nights.choose(null)}><Icon name="back" size={16} />All days</button>
    </nav>
{:else if nights.campsites}
    <div class="keep">
        <p id="keep-campsites"><strong>{nights.campsites} {nights.campsites === 1 ? 'campsite' : 'campsites'} suggested</strong></p>
        <button type="button" class="keep-all" aria-describedby="keep-campsites" onclick={() => nights.keepCampsites()}>{nights.campsites === 1 ? 'Keep it' : nights.campsites === 2 ? 'Keep both' : `Keep all ${nights.campsites}`}</button>
    </div>
{/if}
<div class="itinerary" bind:this={root}>
    {#if focused}
        {@const conflict = conflictOf(focused.number)}
        <DayCard {trip} day={focused} from={fromOf(focused)} to={toOf(focused)} {theme} ascentKnown={line?.unknownElevationKm === 0} {nights}
            conflict={conflict ? conflict[0].night! : null} profile={profileOf(focused)} onShowConflict={() => conflict && onShowConflict(conflict)} />
        <DayLine {trip} {line} day={focused} {days} from={fromOf(focused)} {theme} {nights} {revealId} {hoveredId} {onHover} {onInspect} onShowEnd={() => onShowEnd(focused)} />
    {:else}
        {#each visible as day (day.number)}
            {@const conflict = conflictOf(day.number)}
            <div class="slot" data-glide in:fade={{ duration: ms(160) }} out:fade={{ duration: ms(120) }}>
                <DayCard {trip} day={shown(day)} from={fromOf(day)} to={toOf(day)} foot={chosen === null && day.number !== days.length} flash={revealId === `night-${day.number}`}
                    {theme} ascentKnown={line?.unknownElevationKm === 0} {nights} conflict={conflict ? conflict[0].night! : null} profile={profileOf(shown(day))}
                    onOpen={() => openDay(day.number)} onShowEnd={() => onShowEnd(day)} onShowConflict={() => conflict && onShowConflict(conflict)} />
            </div>
            {#if day.number === chosen}
                <div class="seam" data-glide in:fade={{ duration: ms(180) }} out:fade={{ duration: ms(120) }}>
                    <NightSeam {trip} {line} {day} {theme} {nights} {hoveredId} {onHover} onEnd={value => end = value} />
                </div>
            {/if}
        {/each}
    {/if}
</div>

<style>
    .day-nav {
        position: sticky;
        top: 0;
        z-index: 2;
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 8px;
        padding: 8px 16px;
        border-bottom: 1px solid var(--line);
        background: var(--panel);
    }
    .week {
        position: sticky;
        top: 0;
        z-index: 2;
        display: flex;
        align-items: center;
        gap: 8px;
        height: 36px;
        padding: 0 12px;
        border-bottom: 1px solid var(--line);
        background: var(--panel);
        font-size: 13px;
    }
    .week strong, .stepper strong { font: 600 14px var(--sans); }
    .week span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ink-soft); }
    .week .icon { width: 28px; height: 28px; border: 1px solid var(--line-strong); }
    .overview, .stepper { display: flex; align-items: center; gap: 8px; }
    .overview { min-height: 36px; padding: 6px 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font-size: 13px; font-weight: 600; }
    .overview:hover { background: var(--parchment-2); }
    .stepper { gap: 2px; }
    .keep { display: flex; align-items: center; gap: 12px; margin: 4px 12px 8px; padding: 10px 12px; border-radius: 8px; background: var(--parchment-2); }
    .keep p { flex: 1; min-width: 0; margin: 0; }
    .keep strong { font: 600 14px/1.3 var(--sans); }
    .keep-all { flex: none; min-height: 32px; padding: 0 14px; border: 0; border-radius: 6px; background: var(--amber); color: var(--on-amber); font: inherit; font-weight: 600; cursor: pointer; }
    .keep-all:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }

    .itinerary {
        position: relative;
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 4px 12px 16px;
    }
    /* A card glides over a card that leaves and over the seam. */
    .slot { position: relative; z-index: 1; }
    .slot:global([inert]) { z-index: 0; }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    .icon {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .icon:hover:not(:disabled) {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .icon:disabled { opacity: .4; cursor: default; }
</style>
