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
    import { canMoveLoopStart, hasEndpoints as endpointsChosen, maxRidingDays, planView, provisionalDays, type Day, type LegMode, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let {
        kind, trip, line, days, detailsError = '', dayLabels, night, point, place, coordinate, legMode,
        onClose, onEndpoint, onAddHere, onLegMode, onInsert, onLoop, onLoopStart, onPick, onChooseNight, onStay, onAddVisit, onRename, onKind, onRemove, children,
    }: {
        detailsError?: string;
        kind: CalloutKind;
        trip: Trip;
        /** The line of the trip, once it is calculated. */
        line?: RoutingLine;
        days: Day[];
        /** Riding number → calendar number. */
        dayLabels: Record<number, number>;
        /** The riding day the rider works on. */
        night: number;
        point?: RoutePoint;
        /** The place of a place callout, or the mapped place a point was made from. */
        place?: Place;
        /** Where a place callout would pin the overnight. */
        coordinate?: Coordinate | null;
        legMode: LegMode;
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

    const hasEndpoints = $derived(endpointsChosen(trip));
    const multi = $derived(trip.mode !== 'route');
    const types = $derived(([
        { value: 'via', label: 'Shape', icon: 'route' },
        { value: 'waypoint', label: 'Visit', icon: 'flag' },
        { value: 'night', label: 'Sleep', icon: 'camp' },
        { value: 'marker', label: 'Marker', icon: 'pin' },
    ] as { value: EditableKind; label: string; icon: string }[]).filter(type => multi || type.value !== 'night'));
    const legModes: { value: LegMode; label: string }[] = [
        { value: 'routed', label: 'Follow roads' },
        { value: 'straight', label: 'Straight lines' },
        { value: 'drawn', label: 'Freehand' },
        { value: 'transfer', label: 'Transfer (not ridden)' },
    ];

    let renaming = $state(false);
    let renameButton = $state<HTMLButtonElement>();
    // svelte-ignore state_referenced_locally
    let sleepDay = $state(night);

    const sleeps = $derived(hasEndpoints && days.length > 0 && multi);
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

{#snippet endpoints()}
    {#if onEndpoint}<div class="add-types endpoints">
        <button type="button" onclick={() => onEndpoint?.('start')}><Icon name="pin" size={15} />Start here</button>
        <button type="button" onclick={() => onEndpoint?.('finish')}><Icon name="flag" size={15} />Finish here</button>
    </div>{/if}
{/snippet}
<Callout label="Map details" {onClose}>
    {#if kind === 'add'}
        <h2>{hasEndpoints ? 'Add point here' : 'Plan from here'}</h2>
        {@render endpoints()}
        {#if hasEndpoints}
        <div class="add-types">
            {#each types.filter(type => type.value !== 'marker') as type (type.value)}
                <button type="button" onclick={() => onAddHere(type.value)}><Icon name={type.icon} size={15} />{type.label}</button>
            {/each}
        </div>
        <button type="button" class="quiet" onclick={() => onAddHere('marker')}><Icon name="pin" size={15} />Add a marker</button>
        {/if}
    {:else if kind === 'leg'}
        <h2>This leg</h2>
        <Segmented label="Leg mode" columns={2} options={legModes} value={legMode} onChange={onLegMode} />
        <p class="hint">Straight lines join shaping points without following roads.</p>
        <button type="button" class="secondary" onclick={onInsert}>Insert point here</button>
        {#if canMoveLoopStart(trip)}
            <button type="button" class="secondary" onclick={onLoopStart}>Start the loop here</button>
            <p class="hint">The start moves to this point. The stops keep their order.</p>
        {/if}
    {:else if kind === 'dayend'}
        <h2>Day {dayLabels[night]} ends here for now</h2>
        <button type="button" class="secondary" onclick={onChooseNight}>Choose where to sleep</button>
        <button type="button" class="planner-action" onclick={onPick}>Pick another spot on the map</button>
        <p class="hint">Drag the marker along the route to move the day end.</p>
    {:else if kind === 'place'}
        {#if place}
            <PlaceDetails {place} error={detailsError} />
            {@render endpoints()}
        {:else}
            <h2 class="spot">Overnight spot</h2>
        {/if}
        {#if sleeps}
            <label class="field">End of day
                <select bind:value={sleepDay}>
                    {#each days as day (day.number)}
                        <option value={day.number} disabled={day.number >= maxRidingDays}>Day {dayLabels[day.number]}{day.pinned ? ` · replaces ${day.pinned.label}` : day.number >= maxRidingDays ? ' · day limit reached' : day.number === days.length ? ' · adds a night' : ''}</option>
                    {/each}
                </select>
            </label>
            {#if preview}
                <p class="predict">Day {dayLabels[sleepDay]} would be ≈ <strong>{distanceIn(preview.distance * 1000).toFixed(1)} {settings.units} ↑ {Math.round(heightIn(preview.ascent))} {heightUnit()}</strong></p>
            {/if}
            {#if sleepDay >= maxRidingDays}<p class="hint">A trip can have up to {maxRidingDays} riding days. Choose an earlier day for this overnight.</p>{/if}
            <button type="button" class="primary" disabled={sleepDay >= maxRidingDays} onclick={() => onStay(sleepDay)}>{days[sleepDay - 1]?.pinned ? 'Replace overnight' : 'Stay here'}<Icon name="check" size={15} /></button>
            {#if place}<button type="button" class="secondary" onclick={() => onAddVisit(place)}>Add as visit</button>{/if}
        {:else if place && hasEndpoints}
            <button type="button" class="primary" onclick={() => onAddVisit(place)}>{place.category === 'peak' ? 'Ride over it' : 'Add visit'}<Icon name="plus" size={15} /></button>
        {/if}
    {:else if kind === 'point' && point}
        <div class="title">
            <h2>{point.kind === 'via' ? 'Shaping point' : point.label}</h2>
            {#if point.kind !== 'via'}
                <button type="button" class="icon" bind:this={renameButton} aria-label="Rename point" aria-expanded={renaming} onclick={() => renaming = !renaming}><Icon name="pencil" size={15} /></button>
            {/if}
        </div>
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
        {/if}
        {#if place}<PlaceDetails {place} titled={false} error={detailsError} />{/if}
        {#if hasEndpoints && point.kind !== 'start' && point.kind !== 'finish'}
            <Segmented label="Point type" options={types} columns={types.length > 3 ? 2 : 0} value={point.kind === 'detour' ? 'waypoint' : point.kind as EditableKind} onChange={onKind} />
        {/if}
        {#if point.kind === 'finish' && hasEndpoints}
            <button type="button" class="secondary" onclick={onLoop}>Back to start</button>
            <p class="hint">The route returns to the start, and this point becomes the last stop.</p>
        {/if}
        <button type="button" class="quiet" onclick={onRemove}><Icon name="trash" size={15} />Remove point</button>
    {/if}
    {@render children?.()}
</Callout>

<style>
    h2 {
        margin: 0 28px 12px 0;
        font: 600 14px var(--sans);
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    /* As the name of a place, which this title stands for. */
    .spot {
        margin-bottom: 5px;
        font-size: 17px;
        line-height: 1.3;
    }
    .icon:hover {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .title {
        display: flex;
        align-items: center;
        gap: 4px;
        margin: 0 28px 12px 0;
    }
    .title h2 {
        margin: 0;
    }
    .icon {
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .endpoints { margin-bottom: 12px; }
    .add-types {
        display: flex;
        gap: 8px;
    }
    .add-types button {
        flex: 1;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        height: 36px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        font-weight: 600;
    }
    .add-types button:hover {
        background: var(--parchment-2);
    }
    .quiet {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        margin-top: 12px;
        padding: 4px 0;
        color: var(--ink-soft);
    }
    .quiet:hover {
        color: var(--ink);
    }
    .rename {
        width: 100%;
        height: 32px;
        margin-bottom: 12px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font-size: 14px;
    }
    .hint {
        margin: 4px 0 0;
        color: var(--ink-soft);
    }
    .field {
        display: flex;
        flex-direction: column;
        gap: 4px;
        color: var(--ink-soft);
    }
    .field select {
        height: 32px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
    }
    .predict {
        margin: 12px 0 0;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .predict strong {
        font-weight: 600;
        color: var(--ink);
    }
    .primary,
    .secondary {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 8px;
        width: 100%;
        height: 36px;
        margin-top: 12px;
        border-radius: 6px;
        font-weight: 600;
    }
    .primary {
        background: var(--amber);
        color: var(--on-amber);
    }
    .primary:hover {
        filter: brightness(.95);
    }
    .secondary {
        margin-top: 8px;
        border: 1px solid var(--line-strong);
    }
    .secondary:hover {
        background: var(--parchment-2);
    }
</style>
