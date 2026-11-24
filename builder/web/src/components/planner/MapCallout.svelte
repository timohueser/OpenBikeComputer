<script lang="ts" module>
    import type { PointKind } from '../../lib/planner/editor';

    export type CalloutKind = 'point' | 'place' | 'dayend' | 'add' | 'leg';
    export type EditableKind = Extract<PointKind, 'via' | 'pass' | 'waypoint' | 'night' | 'marker'>;
</script>

<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Callout from './Callout.svelte';
    import PlaceDetails from './PlaceDetails.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { canMoveLoopStart, hasEndpoints as endpointsChosen, maxRidingDays, multiDay, planView, provisionalDays, type Day, type LegMode, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let {
        kind, trip, line, days, detailsError = '', night, point, place, coordinate, legMode, takeFocus = true,
        onClose, onEndpoint, onAddHere, onLegMode, onInsert, onLoop, onLoopStart, onPick, onChooseNight, onStay, onAddVisit, onRename, onKind, onRemove, children,
    }: {
        detailsError?: string;
        kind: CalloutKind;
        trip: Trip;
        /** The line of the trip, once it is calculated. */
        line?: RoutingLine;
        days: Day[];
        /** The day the rider works on. */
        night: number;
        point?: RoutePoint;
        /** The place of a place callout, or the mapped place a point was made from. */
        place?: Place;
        /** Where a place callout would pin the overnight. */
        coordinate?: Coordinate | null;
        legMode: LegMode;
        /** False keeps the focus where it is, such as in a radio group that moves on with the arrow keys. */
        takeFocus?: boolean;
        onEndpoint?: (kind: 'start' | 'finish') => void;
        onClose: () => void;
        onAddHere: (kind: EditableKind) => void;
        onLegMode: (mode: LegMode) => void;
        onInsert: () => void;
        /** "Back to start" on the finish. */
        onLoop?: () => void;
        /** "Start the loop here" on the line of a loop. */
        onLoopStart?: () => void;
        onPick: () => void;
        /** Opens the towns to sleep in for the night of this day end. */
        onChooseNight: () => void;
        onStay: (ridingDay: number) => void;
        onAddVisit: (place: Place) => void;
        onRename: (label: string) => void;
        onKind: (kind: EditableKind | 'detour') => void;
        onRemove: () => void;
        /** Extra details below the callout actions, such as a data layer at this spot. */
        children?: Snippet;
    } = $props();

    /** A row has its name for a screen reader, and its note as the description. */
    const uid = $props.id();
    /** The triangle of the start, as on its row of the list. */
    const START = 'M8 5v14l11-7z';
    const hasEndpoints = $derived(endpointsChosen(trip));
    const hasStart = $derived(trip.points.some(p => p.kind === 'start'));
    const multi = $derived(multiDay(trip));
    const types = $derived(([
        { value: 'via', label: 'Shape', icon: 'route', note: 'The route passes here' },
        { value: 'waypoint', label: 'Visit', icon: 'flag', note: 'A stop on the route' },
        { value: 'night', label: 'Sleep', icon: 'camp', note: 'The end of a day' },
        { value: 'marker', label: 'Marker', icon: 'pin', note: 'A note beside the route' },
    ] as { value: EditableKind; label: string; icon: string; note: string }[]).filter(type => multi || type.value !== 'night'));
    const legModes: { value: LegMode; label: string }[] = [
        { value: 'routed', label: 'Follow roads' },
        { value: 'straight', label: 'Straight line' },
        { value: 'drawn', label: 'Freehand' },
    ];

    let renaming = $state(false);
    let renameButton = $state<HTMLButtonElement>();
    // svelte-ignore state_referenced_locally
    let sleepDay = $state(night);

    const sleeps = $derived(hasEndpoints && days.length > 0 && multi);
    /** A spot picked for the night, or a place with beds or pitches: the night is its likely use. */
    const sleepFirst = $derived(!place || ['camp', 'hotel', 'shelter'].includes(place.category));
    const preview = $derived.by(() => {
        if (!coordinate || sleepDay >= days.length) return null;
        return provisionalDays(trip, line, sleepDay, nearestProgress(planView(trip, line).coordinates, coordinate))[sleepDay - 1];
    });

    function rename(label: string) {
        if (!label.trim()) return;
        onRename(label.trim());
        renaming = false;
    }
</script>

<!-- Before the route has its two ends, they are the next step. After that, they move an end. -->
{#snippet endpoints()}
    {#if onEndpoint && hasEndpoints}
        <div class="rows two rule">
            <button type="button" class="row" onclick={() => onEndpoint('start')}><Icon path={START} size={16} /><span>Start here</span></button>
            <button type="button" class="row" onclick={() => onEndpoint('finish')}><Icon name="flag" size={16} /><span>Finish here</span></button>
        </div>
    {:else if onEndpoint}
        <div class="buttons">
            <button type="button" class={['planner-action', !hasStart && 'primary']} onclick={() => onEndpoint('start')}><Icon path={START} size={15} />Start here</button>
            <button type="button" class={['planner-action', hasStart && 'primary']} onclick={() => onEndpoint('finish')}><Icon name="flag" size={15} />Finish here</button>
        </div>
    {/if}
{/snippet}
{#snippet visit()}
    {#if place}
        <button type="button" class={['planner-action', !(sleeps && sleepFirst) && 'primary']} onclick={() => onAddVisit(place)}><Icon name="flag" size={15} />{place.category === 'peak' ? 'Ride over it' : 'Add visit'}</button>
    {/if}
{/snippet}
{#snippet stay()}
    {#if sleeps}
        <div class="buttons">
            <button type="button" class={['planner-action', sleepFirst && 'primary']} disabled={sleepDay >= maxRidingDays} onclick={() => onStay(sleepDay)}><Icon name="camp" size={15} />{days[sleepDay - 1]?.pinned ? 'Replace overnight' : 'Stay here'}</button>
            <select class="day" aria-label="End of day" bind:value={sleepDay}>
                {#each days as day (day.number)}
                    <option value={day.number} disabled={day.number >= maxRidingDays}>Day {day.number}{day.pinned ? ` · replaces ${day.pinned.label}` : day.number >= maxRidingDays ? ' · day limit reached' : day.number === days.length ? ' · adds a night' : ''}</option>
                {/each}
            </select>
        </div>
        {#if sleepDay >= maxRidingDays}<p class="meta">A trip can have up to {maxRidingDays} riding days. Choose an earlier day for this overnight.</p>
        {:else if preview}<p class="meta predict">Day {sleepDay} would be ≈ <strong>{distanceIn(preview.distance * 1000).toFixed(1)} {settings.units} ↑ {Math.round(heightIn(preview.ascent))} {heightUnit()}</strong></p>{/if}
    {/if}
{/snippet}
{#snippet title()}
    {#if point}
        <div class="title">
            {#if renaming}
                <!-- svelte-ignore a11y_autofocus -->
                <input class="rename" aria-label="Point name" value={point.label} autofocus
                    onchange={(event) => rename(event.currentTarget.value)}
                    onkeydown={(event) => {
                        if (event.key === 'Enter') rename(event.currentTarget.value);
                        if (event.key === 'Escape') {
                            // Removing a focused input may fire its change event; the old value keeps the cancelled name out.
                            event.currentTarget.value = point.label;
                            renaming = false;
                        }
                        if (!renaming) tick().then(() => renameButton?.focus());
                    }} />
            {:else}
                <h2>{point.kind === 'via' ? 'Shaping point' : point.label}</h2>
            {/if}
            {#if point.kind !== 'via'}
                <button type="button" class="icon" bind:this={renameButton} aria-label="Rename point" aria-expanded={renaming} onclick={() => renaming = !renaming}><Icon name="pencil" size={15} /></button>
            {/if}
        </div>
    {/if}
{/snippet}
<Callout label="Map details" {onClose} {takeFocus}>
    {#if kind === 'add'}
        <header><h2>{hasEndpoints ? 'Add point here' : 'Plan from here'}</h2></header>
        {#if hasEndpoints}
            <div class="rows">
                {#each types as type (type.value)}
                    <button type="button" class="row" aria-label={type.label} aria-describedby="{uid}-{type.value}" onclick={() => onAddHere(type.value)}><Icon name={type.icon} size={16} /><span>{type.label}</span><small id="{uid}-{type.value}">{type.note}</small></button>
                {/each}
            </div>
        {/if}
        {@render endpoints()}
    {:else if kind === 'leg'}
        <header><h2>This leg</h2></header>
        <Segmented label="Leg mode" options={legModes} value={legMode} onChange={onLegMode} />
        <div class="rows">
            <button type="button" class="row" onclick={onInsert}><Icon name="route" size={16} /><span>Add shaping point</span></button>
            {#if canMoveLoopStart(trip)}
                <button type="button" class="row" aria-label="Start the loop here" aria-describedby="{uid}-loop" onclick={onLoopStart}><Icon name="loop" size={16} /><span>Start the loop here</span><small id="{uid}-loop">The stops keep their order</small></button>
            {/if}
        </div>
    {:else if kind === 'dayend'}
        <header>
            <h2>Day {night} ends here for now</h2>
            <p class="meta">Drag the marker along the route to move it.</p>
        </header>
        <div class="buttons stack">
            <button type="button" class="planner-action primary" onclick={onChooseNight}>Choose where to sleep</button>
            <button type="button" class="planner-action" onclick={onPick}>Pick a spot on the map</button>
        </div>
    {:else if kind === 'place'}
        {#if place}
            <PlaceDetails {place} error={detailsError} />
        {:else}
            <header><h2>Overnight spot</h2></header>
        {/if}
        {#if !hasEndpoints}
            {#if place}<footer>{@render endpoints()}</footer>{/if}
        {:else if place || sleeps}
            <footer>
                {#if sleepFirst}{@render stay()}{@render visit()}{:else}{@render visit()}{@render stay()}{/if}
            </footer>
            {#if place}{@render endpoints()}{/if}
        {/if}
    {:else if kind === 'point' && point}
        {#if place}
            <PlaceDetails {place} {title} error={detailsError} />
        {:else}
            <header>{@render title()}</header>
        {/if}
        {#if hasEndpoints && point.kind !== 'start' && point.kind !== 'finish'}
            <Segmented label="Point type" options={types.map(({ value, label }) => ({ value, label }))} value={point.kind === 'detour' ? 'waypoint' : point.kind as EditableKind} onChange={onKind} />
        {/if}
        <div class="rows">
            {#if point.kind === 'finish' && hasEndpoints}
                <button type="button" class="row" aria-label="Back to start" aria-describedby="{uid}-loop" onclick={onLoop}><Icon name="loop" size={16} /><span>Back to start</span><small id="{uid}-loop">Makes the route a loop</small></button>
            {/if}
            <button type="button" class="row danger" onclick={onRemove}><Icon name="trash" size={16} /><span>Remove point</span></button>
        </div>
    {/if}
    {@render children?.()}
</Callout>

<style>
    .title {
        display: flex;
        align-items: center;
        gap: 4px;
        min-height: 28px;
        margin: -4px 28px -4px 0;
    }
    .title :global(h2) {
        margin: 0;
    }
    .icon {
        flex: none;
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        cursor: pointer;
    }
    .icon:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    /* The field takes the place of the title, with its text where the title was. */
    .rename {
        flex: 1;
        min-width: 0;
        height: 28px;
        margin-left: -7px;
        padding: 0 6px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 16px var(--sans);
    }
    .rows.two {
        grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .stack {
        flex-direction: column;
    }
    /* The day of the night, beside the button that ends it here. */
    .day {
        flex: 0 1 auto;
        min-width: 0;
        max-width: 50%;
        height: 36px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
        cursor: pointer;
    }
    .predict {
        font-variant-numeric: tabular-nums;
    }
    .predict strong {
        font-weight: 600;
        color: var(--ink);
    }
</style>
