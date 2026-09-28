<script lang="ts" module>
    import type { PointKind } from '../../lib/planner/editor';

    export type CalloutKind = 'point' | 'place' | 'dayend' | 'add' | 'leg';
    export type EditableKind = Extract<PointKind, 'via' | 'pass' | 'waypoint' | 'night' | 'marker'>;
</script>

<script lang="ts">
    import { onMount } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import PlaceRow from './PlaceRow.svelte';
    import Segmented from './Segmented.svelte';
    import { categoryLabels } from '../../lib/planner/poi-kinds';
    import { profileAscent } from '../../lib/planner/profile-data';
    import { dayOverTarget, pinNight, tripDays, type Coordinate, type Day, type LegMode, type OvernightCandidate, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';

    let {
        kind, trip, days, dayLabels, night, point, place, coordinate, candidates, legMode,
        onClose, onAddHere, onLegMode, onInsert, onPick, onSelectPlace, onStay, onAddVisit, onRename, onKind, onRemove,
    }: {
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

    const multi = $derived(trip.mode !== 'route');
    const types = $derived(([
        { value: 'via', label: 'Shape', icon: 'route' },
        { value: 'waypoint', label: 'Visit', icon: 'flag' },
        { value: 'night', label: 'Sleep', icon: 'camp' },
        { value: 'marker', label: 'Marker', icon: 'pin' },
    ] as { value: EditableKind; label: string; icon: string }[]).filter(type => multi || type.value !== 'night'));
    const legModes: { value: LegMode; label: string }[] = [
        { value: 'routed', label: 'Routed' },
        { value: 'straight', label: 'Straight' },
        { value: 'drawn', label: 'Drawn' },
    ];

    let root: HTMLDivElement;
    let renaming = $state(false);
    // svelte-ignore state_referenced_locally
    let sleepDay = $state(night);

    const sleeps = $derived(multi && (!place || place.category === 'hotel' || place.category === 'camp'));
    const preview = $derived.by(() => {
        if (!coordinate || sleepDay >= days.length) return null;
        const day = tripDays(pinNight(trip, sleepDay, coordinate, 'Preview'))[sleepDay - 1];
        const ascent = profileAscent(day.from, day.to);
        const over = dayOverTarget(trip, day, ascent);
        return { distance: day.distance, ascent, over: over.km > 0 || over.climb > 0 };
    });

    onMount(() => {
        // The map moves this content into its popup after mounting.
        const frame = requestAnimationFrame(() => root.focus({ preventScroll: true }));
        return () => cancelAnimationFrame(frame);
    });

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || (event.target as HTMLElement).closest('input, select')) return;
        event.stopPropagation();
        onClose();
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
    <button type="button" class="close" onclick={onClose} aria-label="Close"><Icon name="close" size={15} /></button>
    {#if kind === 'add'}
        <h2>Add point here</h2>
        <div class="add-types">
            {#each types.filter(type => type.value !== 'marker') as type (type.value)}
                <button type="button" onclick={() => onAddHere(type.value)}><Icon name={type.icon} size={15} />{type.label}</button>
            {/each}
        </div>
        <button type="button" class="quiet" onclick={() => onAddHere('marker')}><Icon name="pin" size={15} />Add a marker</button>
    {:else if kind === 'leg'}
        <h2>This leg</h2>
        <Segmented label="Leg mode" options={legModes} value={legMode} onChange={onLegMode} />
        <button type="button" class="secondary" onclick={onInsert}>Insert point here</button>
    {:else if kind === 'dayend'}
        <h2>Day {dayLabels[night]} ends here for now</h2>
        <div class="column-head"><small>Day {dayLabels[night]} would be</small></div>
        {#each candidates as candidate (candidate.place.id)}
            <PlaceRow place={candidate.place} day={candidateDay(candidate)} onSelect={onSelectPlace} />
        {/each}
        <button type="button" class="link" onclick={onPick}>Pick another spot on the map</button>
        <p class="hint">Drag the marker along the route to move the day end.</p>
    {:else if kind === 'place'}
        <h2>{place?.label ?? 'Overnight spot'}</h2>
        {#if place}<p class="kind">{categoryLabels[place.category]}</p>{/if}
        {#if sleeps}
            <label class="field">End of day
                <select bind:value={sleepDay}>
                    {#each days as day (day.number)}
                        <option value={day.number}>Day {dayLabels[day.number]}{day.pinned ? ` · replaces ${day.pinned.label}` : day.number === days.length ? ' · adds a night' : ''}</option>
                    {/each}
                </select>
            </label>
            {#if preview}
                <p class="predict">Day {dayLabels[sleepDay]} would be <strong class:over={preview.over}>{preview.distance.toFixed(1)} km ↑ {preview.ascent} m</strong></p>
            {/if}
            <button type="button" class="primary" onclick={() => onStay(sleepDay)}>{days[sleepDay - 1]?.pinned ? 'Replace overnight' : 'Stay here'}<Icon name="check" size={15} /></button>
            {#if place}<button type="button" class="secondary" onclick={() => onAddVisit(place)}>Add as visit</button>{/if}
        {:else if place}
            <button type="button" class="primary" onclick={() => onAddVisit(place)}>Add visit<Icon name="plus" size={15} /></button>
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
        {#if point.kind !== 'start' && point.kind !== 'finish'}
            <Segmented label="Point type" options={types} value={point.kind === 'detour' ? 'waypoint' : point.kind as EditableKind} onChange={onKind} />
            {#if point.kind === 'waypoint' || point.kind === 'detour'}
                <div class="gap">
                    <Segmented label="How the route reaches it" value={point.kind} onChange={onKind}
                        options={[{ value: 'waypoint', label: 'Through' }, { value: 'detour', label: 'Out and back' }]} />
                </div>
            {/if}
            <button type="button" class="quiet" onclick={onRemove}><Icon name="trash" size={15} />Remove point</button>
        {/if}
    {/if}
</div>

<style>
    .callout {
        position: relative;
        width: 288px;
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
    .kind {
        margin: -8px 0 12px;
        color: var(--ink-soft);
    }
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
    .gap {
        margin-top: 8px;
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
    .link {
        margin-top: 8px;
        padding: 0;
        color: var(--forest);
        font-weight: 600;
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .hint {
        margin: 8px 0 0;
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
