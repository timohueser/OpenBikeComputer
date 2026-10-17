<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import DayRow from './DayRow.svelte';
    import NightNode from './NightNode.svelte';
    import StopPicker from './StopPicker.svelte';
    import { orderedRoutePoints, type Day, type ItineraryDay, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';
    import { proposalTitle } from '../../lib/planner/stops';
    import type { RoutingLine } from '../../lib/planner/routing';

    let {
        trip, line, itinerary, days, theme, expandedDay, nights, conflicts, selectedId, revealId, hoveredId = null, onHover,
        onToggle, onOverview, onInspect, onShowEnd, onEditTarget, onShowConflict,
        onAddRest, onRemoveRest, onNameRest,
    }: {
        trip: Trip;
        /** The line of the trip, once it is calculated. */
        line?: RoutingLine;
        itinerary: ItineraryDay[];
        days: Day[];
        theme: 'light' | 'dark';
        /** Riding number of the open day. */
        expandedDay: number | null;
        nights: NightChoices;
        conflicts: [RoutePoint, RoutePoint][];
        selectedId: string | null;
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        /** The point whose row lights up for a moment. */
        revealId: string | null;
        onToggle: (ridingDay: number) => void;
        onOverview: () => void;
        onInspect: (point: RoutePoint) => void;
        onShowEnd: (day: Day) => void;
        onEditTarget: () => void;
        onShowConflict: (points: [RoutePoint, RoutePoint]) => void;
        onAddRest: (afterRidingDay: number) => void;
        onRemoveRest: (index: number) => void;
        onNameRest: (index: number, name: string) => void;
    } = $props();

    let root: HTMLDivElement;
    let dayPicker = $state<HTMLSelectElement>();
    let restEditing = $state<number | null>(null);
    const focused = $derived(itinerary.find(day => !day.rest && day.ridingNumber === expandedDay));
    const visible = $derived(focused ? itinerary.filter(day => day.ridingNumber === focused.ridingNumber) : itinerary);
    const ridingDays = $derived(itinerary.filter(day => !day.rest));
    const scale = $derived(Math.max(trip.limit, ...days.map(day => day.distance)) || 1);
    const calendar = $derived(Object.fromEntries(itinerary.filter(d => !d.rest).map(d => [d.ridingNumber, d.number])));
    const ends = $derived(orderedRoutePoints(trip));
    const suggested = $derived(Object.fromEntries(days.flatMap(day => {
        const around = day.pinned ? undefined : nights.around(day.number);
        return around ? [[day.number, proposalTitle(around, nights.sleep)]] : [];
    })));

    $effect(() => {
        expandedDay;
        tick().then(() => {
            const scroll = root?.closest('.pane-scroll');
            if (scroll) scroll.scrollTop = 0;
        });
    });

    async function openDay(riding: number) {
        onToggle(riding);
        await tick();
        dayPicker?.focus({ preventScroll: true });
    }

    async function overview() {
        const riding = expandedDay;
        onOverview();
        await tick();
        root?.querySelector<HTMLElement>(`[data-day="${riding}"]`)?.focus();
    }

    function conflictOf(ridingDay: number) {
        return conflicts.find(([, after]) => after.night === ridingDay) ?? null;
    }

    function nameRest(index: number, name: string) {
        restEditing = null;
        onNameRest(index, name);
    }
</script>

{#if focused}
    <nav class="day-nav" aria-label="Day navigation">
        <button type="button" class="overview" onclick={overview}><Icon name="back" size={16} />All days</button>
        <div class="day-picker">
            <button type="button" class="icon" aria-label="Previous riding day" disabled={focused.ridingNumber === 1} onclick={() => onToggle(focused.ridingNumber - 1)}><Icon name="back" size={16} /></button>
            <select bind:this={dayPicker} aria-label="Selected day" value={focused.ridingNumber} onchange={event => onToggle(Number(event.currentTarget.value))}>
                {#each ridingDays as day}<option value={day.ridingNumber}>Day {day.number}</option>{/each}
            </select>
            <button type="button" class="icon" aria-label="Next riding day" disabled={focused.ridingNumber === days.length} onclick={() => onToggle(focused.ridingNumber + 1)}><Icon name="arrow" size={16} /></button>
        </div>
    </nav>
{/if}
{#snippet towns(night: number, cancel = false)}
    {@const window = nights.window(night)}
    {#if line && window}
        <StopPicker {trip} {line} {night} labels={[calendar[night], calendar[night + 1]]} {theme} found={nights.found(night)} {window} stopId={nights.stopId}
            date={nights.date(night)} elevationAt={nights.elevationAt} onOpen={id => nights.open(id)} onEndHere={stop => nights.endHere(night, stop)}
            onStay={place => nights.stay(night, place)} onSelectPlace={place => nights.selectPlace(place)} onPick={() => nights.pick()} onRetry={() => nights.retry(night)}
            onCancel={cancel ? () => nights.choose(null) : undefined} />
    {:else}
        <p class="hint">The towns to sleep in appear when the route is ready.</p>
    {/if}
{/snippet}

{#if !focused}
    {#if nights.sleep === 'camping' && nights.campsites}
        <div class="keep">
            <p id="keep-campsites"><strong>{nights.campsites} {nights.campsites === 1 ? 'campsite' : 'campsites'} suggested</strong><small>{nights.campsites === 1 ? 'Keep it' : 'Keep them'}, or choose each night yourself.</small></p>
            <button type="button" class="keep-all" aria-describedby="keep-campsites" onclick={() => nights.keepCampsites()}>{nights.campsites === 1 ? 'Keep it' : nights.campsites === 2 ? 'Keep both' : `Keep all ${nights.campsites}`}</button>
        </div>
    {:else}
        <p class="hint">Choose where to sleep each night. Open a day for its stops.</p>
    {/if}
{/if}
<div class="itinerary" class:ladder={!focused} bind:this={root}>
    {#if !focused}
        <div class="endpoint"><span class="end-mark" aria-hidden="true"><Icon path="M8 5v14l11-7z" size={11} /></span><div><strong>{ends[0]?.label}</strong><small>Start</small></div></div>
    {/if}
    {#key expandedDay}
    {#each visible as day (`${day.number}-${day.rest}`)}
        {#if day.rest}
            <div class="rest">
                <span class="badge"><Icon name="pause" size={14} /></span>
                <div class="rest-text">
                    {#if restEditing === day.restIndex}
                        <!-- svelte-ignore a11y_autofocus -->
                        <input aria-label={`Name rest day ${day.number}`} value={trip.restNames?.[day.restIndex!] ?? ''} placeholder="Rest day name" autofocus
                            onblur={(event) => nameRest(day.restIndex!, event.currentTarget.value)}
                            onkeydown={(event) => {
                                if (event.key === 'Enter') event.currentTarget.blur();
                                if (event.key === 'Escape') restEditing = null;
                            }} />
                    {:else}
                        <button type="button" class="rest-name" aria-label={`Name rest day ${day.number}`} onclick={() => restEditing = day.restIndex!}>
                            <strong>Day {day.number} · {trip.restNames?.[day.restIndex!] || 'Rest'}</strong><Icon name="pencil" size={13} />
                        </button>
                    {/if}
                    <small>{day.pinned?.label ?? `Night ${calendar[day.ridingNumber]} not chosen`}</small>
                </div>
                <button type="button" class="icon" aria-label={`Remove rest day ${day.number}`} onclick={() => onRemoveRest(day.restIndex!)}><Icon name="close" size={15} /></button>
            </div>
        {:else}
            {@const conflict = conflictOf(day.ridingNumber)}
            <DayRow
                {trip} {line} {day} {days} {theme} {scale} {selectedId} {revealId} {hoveredId} {onHover} {calendar} {suggested}
                expanded={expandedDay === day.ridingNumber}
                choosing={expandedDay === day.ridingNumber && nights.choosing === day.ridingNumber}
                conflict={conflict ? calendar[conflict[0].night!] : null}
                onToggle={() => openDay(day.ridingNumber)}
                {onInspect} {onEditTarget} onChange={() => nights.choose(day.ridingNumber)}
                onShowEnd={() => onShowEnd(days[day.ridingNumber - 1])}
                onShowConflict={() => conflict && onShowConflict(conflict)}
            >{#snippet picker()}{@render towns(day.ridingNumber, !!days[day.ridingNumber - 1].pinned)}{/snippet}</DayRow>
            {#if !focused && day.ridingNumber < days.length}
                <NightNode day={days[day.ridingNumber - 1]} label={calendar[day.ridingNumber]} {nights} onShow={() => onShowEnd(days[day.ridingNumber - 1])}>{@render towns(day.ridingNumber)}</NightNode>
            {/if}
            {#if focused && day.ridingNumber < days.length && !trip.restAfter?.includes(day.ridingNumber)}
                <button type="button" class="add-rest" onclick={() => onAddRest(day.ridingNumber)}><Icon name="plus" size={13} />Add rest day</button>
            {/if}
        {/if}
    {/each}
    {/key}
    {#if !focused}
        <div class="endpoint"><span class="end-mark" aria-hidden="true"><Icon name="flag" size={12} /></span><div><strong>{trip.loop ? ends[0]?.label : ends.at(-1)?.label}</strong><small>Finish</small></div></div>
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
    .overview, .day-picker { display: flex; align-items: center; gap: 8px; }
    .overview { min-height: 36px; padding: 6px 10px; border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel); color: var(--ink); font-size: 13px; font-weight: 600; }
    .overview:hover { background: var(--parchment-2); }
    .day-picker { gap: 2px; }
    select { height: 34px; padding: 0 4px; border: 0; border-radius: 6px; color: var(--ink); background: var(--panel); font: 600 14px var(--sans); }
    .hint { margin: 4px 8px 8px; font-size: 13px; color: var(--ink-soft); }
    .keep { display: flex; align-items: center; gap: 12px; margin: 4px 8px 8px; padding: 10px 12px; border-radius: 8px; background: var(--parchment-2); }
    .keep p { flex: 1; min-width: 0; margin: 0; }
    .keep strong, .keep small { display: block; }
    .keep strong { font: 600 14px/1.3 var(--sans); }
    .keep small { margin-top: 2px; font-size: 13px; color: var(--ink-soft); }
    .keep-all { flex: none; min-height: 32px; padding: 0 14px; border: 0; border-radius: 6px; background: var(--amber); color: var(--on-amber); font: inherit; font-weight: 600; cursor: pointer; }
    .keep-all:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }

    .itinerary {
        display: flex;
        flex-direction: column;
        gap: 4px;
        padding: 4px 8px 16px;
    }
    /* One rail joins start, days, nights and finish, through the centre of their marks. */
    .ladder {
        position: relative;
        gap: 0;
    }
    .ladder::before {
        content: "";
        position: absolute;
        top: 26px;
        bottom: 38px;
        left: 27px;
        width: 2px;
        border-radius: 1px;
        background: var(--line);
    }
    .ladder :global(.day:not(.expanded)) {
        border-bottom: 0;
    }
    .ladder :global(.badge),
    .ladder :global(.night) {
        position: relative;
        z-index: 1;
    }
    .ladder > :global(.night),
    .ladder > :global(.towns),
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
    .rest {
        display: flex;
        align-items: center;
        gap: 12px;
        padding: 8px;
        color: var(--ink);
    }
    .badge {
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        border: 1.5px dashed var(--line-strong);
        border-radius: 50%;
        color: var(--ink-soft);
    }
    .rest-text {
        flex: 1;
        min-width: 0;
    }
    .rest-name {
        display: flex;
        align-items: center;
        gap: 8px;
        text-align: left;
    }
    .rest-name strong {
        font: 600 14px var(--sans);
    }
    .rest-name :global(svg) {
        color: var(--ink-faint);
    }
    .rest small {
        display: block;
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .rest input {
        width: 100%;
        height: 30px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 14px;
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
    .add-rest {
        display: flex;
        align-items: center;
        gap: 8px;
        align-self: flex-start;
        margin-left: 8px;
        padding: 4px 8px;
        border-radius: 6px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .add-rest:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
</style>
