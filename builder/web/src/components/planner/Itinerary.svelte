<script lang="ts">
    import { tick, untrack } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import DayCard from './DayCard.svelte';
    import DayRow from './DayRow.svelte';
    import StopPicker from './StopPicker.svelte';
    import { orderedRoutePoints, type Day, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { heightRange, profilePaths, profileSamples, sampleSlice } from '../../lib/planner/profile-data';
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
    const ends = $derived(orderedRoutePoints(trip));
    /** The town of each night by the number of its day: of the place that the rider set, else of the suggestion. */
    const nightTowns = $derived(days.map(day => day.pinned ? day.pinned.locality ?? day.pinned.label : nights.around(day.number)?.stop.name));
    const fromOf = (day: Day) => day.number === 1 ? ends[0]?.label ?? '' : nightTowns[day.number - 2] ?? `Night ${day.number - 1}`;
    const toOf = (day: Day) => day.number === days.length ? (trip.loop ? ends[0] : ends.at(-1))?.label ?? '' : nightTowns[day.number - 1] ?? `Night ${day.number}`;
    const samples = $derived(profileSamples(line));
    const heights = $derived(heightRange(samples));
    const profileOf = (day: Day) => samples.length
        ? { ...profilePaths(sampleSlice(samples, day.from, day.to), heights.low, heights.high), view: `${day.from * 1000} 0 ${(day.to - day.from) * 1000} 105` } : undefined;
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
            onOpen={id => nights.open(id)} onEndHere={stop => nights.endHere(night, stop)}
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
<div class="itinerary" class:cards={!focused} bind:this={root}>
    {#key expandedDay}
    {#each visible as day (day.number)}
        {@const conflict = conflictOf(day.number)}
        {#if focused}
            <DayRow
                {trip} {line} {day} {days} {theme} {selectedId} {revealId} {hoveredId} {onHover} {suggested}
                choosing={nights.choosing === day.number}
                conflict={conflict ? conflict[0].night! : null}
                {onInspect} onChange={() => nights.choose(day.number)}
                onShowEnd={() => onShowEnd(day)}
                onShowConflict={() => conflict && onShowConflict(conflict)}
            >{#snippet picker()}{@render towns(day.number, !!day.pinned)}{/snippet}</DayRow>
        {:else}
            <DayCard {trip} {day} from={fromOf(day)} to={toOf(day)} last={day.number === days.length} {theme} ascentKnown={line?.unknownElevationKm === 0} {nights}
                conflict={conflict ? conflict[0].night! : null} profile={profileOf(day)}
                onOpen={() => openDay(day.number)} onShowEnd={() => onShowEnd(day)} onShowConflict={() => conflict && onShowConflict(conflict)} />
        {/if}
    {/each}
    {/key}
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
    .keep { display: flex; align-items: center; gap: 12px; margin: 4px 12px 8px; padding: 10px 12px; border-radius: 8px; background: var(--parchment-2); }
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
    .cards { gap: 8px; padding: 4px 12px 16px; }
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
