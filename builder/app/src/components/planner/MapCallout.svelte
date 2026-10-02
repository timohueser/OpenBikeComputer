<script lang="ts" module>
    import type { PointKind } from '../../lib/planner/editor';

    export type CalloutKind = 'point' | 'place' | 'dayend' | 'add' | 'leg';
    export type EditableKind = Extract<PointKind, 'via' | 'pass' | 'waypoint' | 'night' | 'marker'>;
</script>

<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import OpeningHours from './OpeningHours.svelte';
    import { kindLabel } from '../../lib/planner/search/presentation';
    import Segmented from './Segmented.svelte';
    import { placeCategories } from '../../lib/planner/poi-kinds';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { dayOverTarget, maxRidingDays, nearestProgress, routeCoordinates, tripDays, type Coordinate, type Day, type LegMode, type OvernightCandidate, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';

    let {
        kind, trip, days, overnightNote = '', dayLabels, night, point, place, coordinate, candidates, legMode,
        onClose, onEndpoint, onAddHere, onLegMode, onInsert, onPick, onSelectPlace, onStay, onAddVisit, onRename, onKind, onRemove,
    }: {
        overnightNote?: string;
        kind: CalloutKind;
        trip: Trip;
        days: Day[];
        /** Riding number → calendar number. */
        dayLabels: Record<number, number>;
        /** The riding day the rider works on. */
        night: number;
        point?: RoutePoint;
        place?: Place;
        /** Where a place callout would pin the overnight. */
        coordinate?: Coordinate | null;
        candidates: OvernightCandidate[];
        legMode: LegMode;
        onEndpoint?: (kind: 'start' | 'finish') => void;
        onClose: () => void;
        onAddHere: (kind: EditableKind) => void;
        onLegMode: (mode: LegMode) => void;
        onInsert: () => void;
        onPick: () => void;
        onSelectPlace: (place: Place) => void;
        onStay: (ridingDay: number) => void;
        onAddVisit: (place: Place) => void;
        onRename: (label: string) => void;
        onKind: (kind: EditableKind | 'detour') => void;
        onRemove: () => void;
    } = $props();

    const hasEndpoints = $derived(trip.points.some(p => p.kind === 'start') && trip.points.some(p => p.kind === 'finish'));
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
    ];

    let root: HTMLDivElement;
    let opener: HTMLElement | null = null;
    let renaming = $state(false);
    // svelte-ignore state_referenced_locally
    let sleepDay = $state(night);

    const sleeps = $derived(hasEndpoints && days.length > 0 && multi);
    const preview = $derived.by(() => {
        if (!coordinate || sleepDay >= days.length) return null;
        const day = tripDays(trip, { night: sleepDay, progress: nearestProgress(routeCoordinates(trip), coordinate) })[sleepDay - 1];
        const ascent = profileAscent(day.from, day.to, trip.routing);
        const over = dayOverTarget(trip, day, ascent);
        return { distance: day.distance, ascent, over: over.km > 0 || over.climb > 0 };
    });

    onMount(() => {
        opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        // The map moves this content into its popup after mounting.
        const frame = requestAnimationFrame(() => root.focus({ preventScroll: true }));
        return () => cancelAnimationFrame(frame);
    });

    function close() {
        onClose();
        if (opener?.isConnected) opener.focus({ preventScroll: true });
    }

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || (event.target as HTMLElement).closest('input, select')) return;
        event.stopPropagation();
        close();
    }

    function candidateDay(candidate: OvernightCandidate) {
        const over = dayOverTarget(trip, candidate, candidate.ascent);
        return { distance: candidate.distance, ascent: candidate.ascent, over: over.km > 0 || over.climb > 0 };
    }

    function rename(label: string) {
        if (!label.trim()) return;
        onRename(label.trim());
        renaming = false;
    }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="callout" bind:this={root} tabindex="-1" role="dialog" aria-label="Map details" onkeydown={key}>
    <button type="button" class="close" onclick={close} aria-label="Close"><Icon name="close" size={15} /></button>
    {#snippet endpoints()}
        {#if onEndpoint}<div class="add-types endpoints">
            <button type="button" onclick={() => onEndpoint?.('start')}><Icon name="pin" size={15} />Start here</button>
            <button type="button" onclick={() => onEndpoint?.('finish')}><Icon name="flag" size={15} />Finish here</button>
        </div>{/if}
    {/snippet}
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
        <Segmented label="Leg mode" options={legModes} value={legMode} onChange={onLegMode} />
        <p class="hint">Straight lines join shaping points without following roads.</p>
        <button type="button" class="secondary" onclick={onInsert}>Insert point here</button>
    {:else if kind === 'dayend'}
        <h2>Day {dayLabels[night]} ends here for now</h2>
        {#if overnightNote}<p class="hint" role="status">{overnightNote}</p>{/if}
        {#if candidates.length}<div class="column-head"><small>Day {dayLabels[night]} would be</small></div>{/if}
        {#each candidates as candidate (candidate.place.id)}
            <PlaceRow place={candidate.place} day={candidateDay(candidate)} onSelect={onSelectPlace} />
        {/each}
        <button type="button" class="planner-action" onclick={onPick}>Pick another spot on the map</button>
        <p class="hint">Drag the marker along the route to move the day end.</p>
    {:else if kind === 'place'}
        <div class="place-heading">
            {#if place}<span class="place-symbol"><Icon path={placeCategories[place.category].icon} size={21} /></span>{/if}
            <div><h2>{place?.label ?? 'Overnight spot'}</h2>
                {#if place}<p class="kind">{place.placeKind ? kindLabel(place.placeKind) : placeCategories[place.category].label}{place.locality ? ` · ${place.locality}` : ''}</p>{/if}
            </div>
        </div>
        {#if place?.description && place.description !== placeCategories[place.category].label}<p class="place-note">{place.description}</p>{/if}
        {#if place && (place.openingHours || ['shop','food','pharmacy','hotel','bike'].includes(place.category))}<OpeningHours value={place.openingHours} />{/if}
        {#if place}{@render endpoints()}{/if}
        {#if sleeps}
            <label class="field">End of day
                <select bind:value={sleepDay}>
                    {#each days as day (day.number)}
                        <option value={day.number} disabled={day.number >= maxRidingDays}>Day {dayLabels[day.number]}{day.pinned ? ` · replaces ${day.pinned.label}` : day.number >= maxRidingDays ? ' · day limit reached' : day.number === days.length ? ' · adds a night' : ''}</option>
                    {/each}
                </select>
            </label>
            {#if preview}
                <p class="predict">Day {dayLabels[sleepDay]} would be ≈ <strong class:over={preview.over}>{preview.distance.toFixed(1)} km ↑ {preview.ascent} m</strong></p>
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
                <button type="button" class="icon" aria-label="Rename point" aria-expanded={renaming} onclick={() => renaming = !renaming}><Icon name="pencil" size={15} /></button>
            {/if}
        </div>
        {#if renaming}
            <!-- svelte-ignore a11y_autofocus -->
            <input class="rename" aria-label="Point name" value={point.label} autofocus
                onchange={(event) => rename(event.currentTarget.value)}
                onkeydown={(event) => {
                    if (event.key === 'Enter') rename(event.currentTarget.value);
                    if (event.key === 'Escape') renaming = false;
                }} />
        {/if}
        {#if hasEndpoints && point.kind !== 'start' && point.kind !== 'finish'}
            <Segmented label="Point type" options={types} columns={types.length > 3 ? 2 : 0} value={point.kind === 'detour' ? 'waypoint' : point.kind as EditableKind} onChange={onKind} />
        {/if}
        <button type="button" class="quiet" onclick={onRemove}><Icon name="trash" size={15} />Remove point</button>
    {/if}
</div>

<style>
    /* Never taller than the map it sits on; the content scrolls as a last resort. */
    .callout {
        position: relative;
        width: 340px;
        max-width: calc(100vw - 48px);
        max-height: calc(var(--map-height, 100vh) - 96px);
        overflow-y: auto;
        padding: 16px;
        color: var(--ink);
        font: 400 13px var(--sans);
        outline: none;
    }
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
    .close {
        position: absolute;
        top: 10px;
        right: 10px;
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .close:hover,
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
    .place-heading { display: flex; gap: 12px; align-items: flex-start; margin-right: 16px; }
    .place-heading h2 { margin: 0 12px 5px 0; font-size: 17px; line-height: 1.3; overflow-wrap: anywhere; }
    .place-symbol { display: grid; place-items: center; width: 40px; height: 40px; flex: none; border-radius: 50%; color: var(--query-place); background: color-mix(in srgb, var(--query-place) 10%, var(--panel)); }
    .kind { margin: 0; color: var(--ink-soft); font-size: 13px; line-height: 1.45; }
    .place-note { margin: 10px 0; color: var(--ink-soft); line-height: 1.45; }
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
    .column-head {
        display: flex;
        justify-content: flex-end;
        padding-right: 23px;
    }
    .column-head small {
        font-size: 11px;
        color: var(--ink-soft);
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
    .predict .over {
        color: var(--coral);
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
