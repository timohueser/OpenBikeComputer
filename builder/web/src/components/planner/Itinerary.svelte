<script lang="ts">
    import { tick, untrack } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import DayRow from './DayRow.svelte';
    import NightNode from './NightNode.svelte';
    import StopPicker from './StopPicker.svelte';
    import { orderedRoutePoints, planView, type Day, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { coordinateAt } from '../../lib/planner/geo';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { proposalTitle } from '../../lib/planner/stops';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { step } from '../../lib/planner/week';
    import { formatDistance, formatHeight } from '../../lib/settings/format';

    let {
        trip, line, days, theme, expandedDay, nightPage, nights, conflicts, selectedId, revealId, hoveredId = null, onHover,
        onToggle, onOverview, onInspect, onShowEnd, onShowConflict, week = null, onWeek,
    }: {
        trip: Trip;
        /** The line of the trip, once it is calculated. */
        line?: RoutingLine;
        days: Day[];
        theme: 'light' | 'dark';
        /** Number of the open day. */
        expandedDay: number | null;
        /** Number of the night whose page replaces the day list. */
        nightPage: number | null;
        nights: NightChoices;
        conflicts: [RoutePoint, RoutePoint][];
        selectedId: string | null;
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        /** The point whose row lights up for a moment. */
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
    let page = $state<HTMLDivElement>();
    let dayPicker = $state<HTMLSelectElement>();
    const focused = $derived(days.find(day => day.number === expandedDay));
    const weekDays = $derived(week ? days.slice(week.start - 1, week.end) : days);
    const visible = $derived(focused ? [focused] : weekDays);
    const scale = $derived(Math.max(0, ...days.map(day => day.hours)) || 1);
    const total = $derived(planView(trip, line).total);
    const ends = $derived(orderedRoutePoints(trip));
    const suggested = $derived(Object.fromEntries(days.flatMap(day => {
        const around = day.pinned ? undefined : nights.around(day.number);
        return around ? [[day.number, proposalTitle(around)]] : [];
    })));

    // A number, so that a new week object with the same days does not scroll the list.
    const weekStart = $derived(week?.start);
    $effect(() => {
        void [expandedDay, weekStart];
        tick().then(() => {
            const scroll = root?.closest('.pane-scroll');
            if (scroll) scroll.scrollTop = 0;
        });
    });

    // The day list keeps its scroll position under a night page; a page opened from an open day returns to the top of the
    // list, as leaving the day does. The page and its towns open at the top. A closed page gives the focus back to the
    // button that opens it.
    let shownPage: number | null = null, shownDay: number | null = null, listTop = 0, scroller: Element | null = null;
    const pane = () => scroller ??= [root, page].find(node => node?.isConnected)?.closest('.pane-scroll') ?? null;
    $effect.pre(() => {
        const next = nightPage, day = expandedDay, town = next !== null && nights.stopId !== null;
        untrack(() => {
            const was = shownPage, fromDay = shownDay !== null;
            shownPage = next;
            shownDay = day;
            if (was === next && !town) return;
            if (was === null) listTop = fromDay ? 0 : pane()?.scrollTop ?? 0;
            tick().then(() => {
                const scroll = pane();
                if (next !== null) {
                    if (scroll) scroll.scrollTop = 0;
                    if (was !== next) page?.querySelector<HTMLElement>('.overview')?.focus({ preventScroll: true });
                } else if (!focused) {
                    if (scroll) scroll.scrollTop = listTop;
                    root?.querySelector<HTMLElement>(`[data-night="${was}"]`)?.focus();
                }
            });
        });
    });

    async function openDay(day: number) {
        onToggle(day);
        await tick();
        dayPicker?.focus({ preventScroll: true });
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

{#if week && nightPage === null}
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
        <div class="day-picker">
            <button type="button" class="icon" aria-label="Previous riding day" disabled={focused.number === 1} onclick={() => onToggle(focused.number - 1)}><Icon name="back" size={16} /></button>
            <select bind:this={dayPicker} aria-label="Selected day" value={focused.number} onchange={event => onToggle(Number(event.currentTarget.value))}>
                {#each days as day}<option value={day.number}>Day {day.number}</option>{/each}
            </select>
            <button type="button" class="icon" aria-label="Next riding day" disabled={focused.number === days.length} onclick={() => onToggle(focused.number + 1)}><Icon name="arrow" size={16} /></button>
        </div>
    </nav>
{/if}
{#snippet towns(night: number, cancel = false, back?: string)}
    {@const window = nights.window(night)}
    {#if line && window}
        <StopPicker {trip} {line} {night} {back} {theme} found={nights.found(night)} {window} stopId={nights.stopId} shownId={nights.shownId}
            date={nights.date(night)} elevationAt={nights.elevationAt} onOpen={id => nights.open(id)} onEndHere={stop => nights.endHere(night, stop)}
            onStay={place => nights.stay(night, place)} onSelectPlace={place => nights.selectPlace(place)} onPick={() => nights.pick()} onRetry={() => nights.retry(night)}
            onCancel={cancel ? () => nights.choose(null) : undefined} kinds={nights.kinds(night)} onKinds={kinds => nights.setKinds(night, kinds)}
            onRelease={days[night - 1].pinned || days[night - 1].split ? () => nights.release(night) : undefined} />
    {:else}
        <p class="hint">The towns to sleep in appear when the route is ready.</p>
    {/if}
{/snippet}

{#if nightPage !== null}
    <div class="night-page" bind:this={page}>
        {#if nights.stopId === null}
            <nav class="day-nav" aria-label="Night navigation">
                <button type="button" class="overview" onclick={() => nights.choose(null)}><Icon name="back" size={16} />All days</button>
            </nav>
            <header>
                <h2>Night {nightPage}</h2>
                <p>Where day {nightPage} ends and day {nightPage + 1} starts</p>
            </header>
        {/if}
        <div class="towns">{@render towns(nightPage, false, `Night ${nightPage}`)}</div>
    </div>
{:else}
{#if !focused && nights.campsites}
    <div class="keep">
        <p id="keep-campsites"><strong>{nights.campsites} {nights.campsites === 1 ? 'campsite' : 'campsites'} suggested</strong></p>
        <button type="button" class="keep-all" aria-describedby="keep-campsites" onclick={() => nights.keepCampsites()}>{nights.campsites === 1 ? 'Keep it' : nights.campsites === 2 ? 'Keep both' : `Keep all ${nights.campsites}`}</button>
    </div>
{/if}
<div class="itinerary" class:ladder={!focused} class:from-start={!week || week.start === 1} class:to-finish={!week || week.end === days.length} bind:this={root}>
    {#if !focused && (!week || week.start === 1)}
        <div class="endpoint"><span class="end-mark" aria-hidden="true"><Icon path="M8 5v14l11-7z" size={11} /></span><div><strong>{ends[0]?.label}</strong><small>Start</small></div></div>
    {/if}
    {#key expandedDay}
    {#each visible as day (day.number)}
        {@const conflict = conflictOf(day.number)}
        <DayRow
            {trip} {line} {day} {days} {theme} {scale} {selectedId} {revealId} {hoveredId} {onHover} {suggested}
            expanded={expandedDay === day.number}
            choosing={expandedDay === day.number && nights.choosing === day.number}
            conflict={conflict ? conflict[0].night! : null}
            onToggle={() => openDay(day.number)}
            {onInspect} onChange={() => nights.choose(day.number)}
            onShowEnd={() => onShowEnd(day)}
            onShowConflict={() => conflict && onShowConflict(conflict)}
        >{#snippet picker()}{@render towns(day.number, !!day.pinned)}{/snippet}</DayRow>
        {#if !focused && day.number < days.length}
            <NightNode {day} {nights} end={line ? coordinateAt(line.coordinates, day.to) : undefined}
                elevation={nights.elevationAt(day.to * total)} onShow={() => onShowEnd(day)} />
        {/if}
    {/each}
    {/key}
    {#if !focused && (!week || week.end === days.length)}
        <div class="endpoint"><span class="end-mark" aria-hidden="true"><Icon name="flag" size={12} /></span><div><strong>{trip.loop ? ends[0]?.label : ends.at(-1)?.label}</strong><small>Finish</small></div></div>
    {/if}
</div>
{/if}

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
    .week strong { font: 600 14px var(--sans); }
    .week span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--ink-soft); }
    .week .icon { width: 28px; height: 28px; border: 1px solid var(--line-strong); }
    .week .icon:disabled { opacity: .4; cursor: default; }
    .overview, .day-picker { display: flex; align-items: center; gap: 8px; }
    .overview { min-height: 36px; padding: 6px 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font-size: 13px; font-weight: 600; }
    .overview:hover { background: var(--parchment-2); }
    .day-picker { gap: 2px; }
    select { height: 34px; padding: 0 4px; border: 0; border-radius: 6px; color: var(--ink); background: var(--panel); font: 600 14px var(--sans); }
    .hint { margin: 4px 8px 8px; font-size: 13px; color: var(--ink-soft); }
    .night-page header { padding: 12px 16px 8px; }
    .night-page h2 { margin: 0; font: 600 17px/1.3 var(--sans); }
    .night-page header p { margin: 2px 0 0; font-size: 13px; color: var(--ink-soft); }
    .towns { padding: 4px 16px 16px; }
    .keep { display: flex; align-items: center; gap: 12px; margin: 4px 8px 8px; padding: 10px 12px; border-radius: 8px; background: var(--parchment-2); }
    .keep p { flex: 1; min-width: 0; margin: 0; }
    .keep strong { font: 600 14px/1.3 var(--sans); }
    .keep-all { flex: none; min-height: 32px; padding: 0 14px; border: 0; border-radius: 6px; background: var(--amber); color: var(--on-amber); font: inherit; font-weight: 600; cursor: pointer; }
    .keep-all:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }

    .itinerary {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 4px 8px 16px;
    }
    /* One rail joins start, days, nights and finish, through the centre of their marks. Without the start or the finish
     * row, it runs from the edge of the first or the last row shown. */
    .ladder {
        position: relative;
        gap: 0;
    }
    .ladder::before {
        content: "";
        position: absolute;
        top: 4px;
        bottom: 16px;
        left: 27px;
        width: 2px;
        border-radius: 1px;
        background: var(--line);
    }
    .ladder.from-start::before { top: 26px; }
    .ladder.to-finish::before { bottom: 38px; }
    .ladder :global(.day:not(.expanded)) {
        border-bottom: 0;
    }
    .ladder :global(.badge),
    .ladder :global(.night) {
        position: relative;
        z-index: 1;
    }
    .ladder > :global(.night),
    .endpoint {
        padding-left: 8px;
    }
    .endpoint {
        display: flex;
        align-items: center;
        gap: 12px;
        min-height: 44px;
    }
    .end-mark {
        position: relative;
        z-index: 1;
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        border-radius: 50%;
        background: var(--ink);
        color: var(--panel);
    }
    .endpoint strong,
    .endpoint small {
        display: block;
    }
    .endpoint strong {
        font: 600 14px/1.3 var(--sans);
        overflow-wrap: anywhere;
    }
    .endpoint small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
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
    .icon:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
</style>
