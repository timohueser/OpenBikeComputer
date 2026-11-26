<script lang="ts" module>
    import type { PointKind } from '../../lib/planner/editor';

    export type CalloutKind = 'point' | 'place' | 'dayend' | 'add' | 'leg';
    export type EditableKind = Extract<PointKind, 'via' | 'pass' | 'waypoint' | 'night' | 'marker'>;
</script>

<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import Callout, { type Hint } from './Callout.svelte';
    import PlaceDetails from './PlaceDetails.svelte';
    import { radioKey } from '../../lib/ui/Segmented.svelte';
    import { dayColor } from '../../lib/planner/day-colors';
    import { canMoveLoopStart, hasEndpoints as endpointsChosen, maxRidingDays, multiDay, planView, provisionalDays, type Day, type LegMode, type Place, type RoutePoint, type Trip } from '../../lib/planner/editor';
    import { nearestProgress, type Coordinate } from '../../lib/planner/geo';
    import type { RoutingLine } from '../../lib/planner/routing';
    import { distanceIn, heightIn, heightUnit } from '../../lib/settings/format';
    import { settings } from '../../lib/settings/settings.svelte';

    let {
        kind, trip, line, days, detailsError = '', night, point, place, coordinate, legMode, takeFocus = true,
        onClose, onEndpoint, onAddHere, onLegMode, onInsert, onLoop, onLoopStart, onPick, onChooseNight, onStay, onAddVisit, onRename, onKind, onRemove, extra,
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
        /** "Start loop here" on the line of a loop. */
        onLoopStart?: () => void;
        onPick: () => void;
        /** Opens the towns to sleep in for the night of this day end. */
        onChooseNight: () => void;
        onStay: (ridingDay: number) => void;
        onAddVisit: (place: Place) => void;
        onRename: (label: string) => void;
        onKind: (kind: EditableKind | 'detour') => void;
        onRemove: () => void;
        /** Extra details over the actions of an add callout, such as a data layer at this spot. */
        extra?: Snippet;
    } = $props();

    /** A tile of the bottom row. `name` is its name for a screen reader when its one word is too short. `note` is its line
     * of the hint, and its description for a screen reader. `go` is the usual action, in amber; the tip of the popup is
     * under the tile with `aim`. */
    type Tile = { id: string; label: string; name?: string; icon?: string; path?: string; note: string; key?: string; go?: boolean; aim?: boolean; danger?: boolean; disabled?: boolean; run: () => void };

    const uid = $props.id();
    /** The triangle of the start, as on its row of the list. */
    const START = 'M8 5v14l11-7z';
    const hasEndpoints = $derived(endpointsChosen(trip));
    const hasStart = $derived(trip.points.some(p => p.kind === 'start'));
    const multi = $derived(multiDay(trip));
    const types = $derived(([
        { value: 'via', label: 'Shape', icon: 'route', note: 'The route passes here.', key: 'S' },
        { value: 'waypoint', label: 'Visit', icon: 'flag', note: 'A stop on the route.', key: 'V' },
        { value: 'night', label: 'Sleep', icon: 'camp', note: 'The end of a day.', key: 'N' },
        { value: 'marker', label: 'Marker', icon: 'pin', note: 'A note beside the route.', key: 'M' },
    ] as { value: EditableKind; label: string; icon: string; note: string; key: string }[]).filter(type => multi || type.value !== 'night'));
    const legModes: { value: LegMode; label: string; icon: string; note: string }[] = [
        { value: 'routed', label: 'Roads', icon: 'route', note: 'The leg follows roads and paths.' },
        { value: 'straight', label: 'Straight', icon: 'straight', note: 'A straight line between its two points.' },
        { value: 'drawn', label: 'Freehand', icon: 'pencil', note: 'Draw the leg on the map.' },
    ];
    const pointIcons: Partial<Record<RoutePoint['kind'], string>> = { via: 'route', night: 'camp', marker: 'pin', pass: 'mountain' };

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
    const figures = (day: { distance: number; ascent: number }) => `${distanceIn(day.distance * 1000).toFixed(1)} ${settings.units} ↑ ${Math.round(heightIn(day.ascent))} ${heightUnit()}`;

    const startTile = (short: boolean): Tile => ({ id: 'start', label: short ? 'Start' : 'Start here', name: 'Start here', path: START, note: 'The route starts here.', run: () => onEndpoint?.('start') });
    const finishTile = (short: boolean): Tile => ({ id: 'finish', label: short ? 'Finish' : 'Finish here', name: 'Finish here', icon: 'flag', note: 'The route ends here.', run: () => onEndpoint?.('finish') });
    /** The tiles of a place in their fixed order: Visit, Sleep, Start, Finish. Before the route has its two ends, only the ends. */
    const placeTiles = $derived.by((): Tile[] => {
        const ends = place && onEndpoint ? [startTile(true), finishTile(true)] : [];
        if (!hasEndpoints) return ends;
        return [
            ...place ? [{ id: 'visit', label: place.category === 'peak' ? 'Ride over it' : 'Visit', name: place.category === 'peak' ? undefined : 'Add visit', icon: 'flag', note: 'A stop on the route.', key: 'V', run: () => onAddVisit(place) }] : [],
            ...sleeps ? [{ id: 'sleep', label: 'Sleep', name: 'Sleep here', icon: 'camp', note: `Day ${sleepDay} ends here.`, key: 'N', disabled: sleepDay >= maxRidingDays, run: () => onStay(sleepDay) }] : [],
            ...ends,
        ];
    });
    /** The usual tile of a place: the missing end, the night at a place to sleep, else the visit. */
    const usual = $derived(!hasEndpoints ? (hasStart ? 'finish' : 'start') : sleeps && sleepFirst ? 'sleep' : place ? 'visit' : 'sleep');

    function rename(label: string) {
        if (!label.trim()) return;
        onRename(label.trim());
        renaming = false;
    }
</script>

{#snippet tile(t: Tile)}
    <button type="button" class={['tile', t.go && 'go', t.danger && 'danger']} aria-label={t.name ?? t.label} aria-describedby="{uid}-{t.id}" aria-keyshortcuts={t.key === 'Delete' ? 'Delete Backspace' : t.key}
        data-hint={t.note} data-key={t.key} data-aim={t.aim ? '' : undefined} disabled={t.disabled} onclick={t.run}>
        <Icon name={t.icon} path={t.path} size={21} />{t.label}<span id="{uid}-{t.id}" hidden>{t.note}</span>
    </button>
{/snippet}
<!-- The line of the part under the pointer, or of the usual tile. A screen reader has it as the description of each tile. -->
{#snippet says(hint: Hint | null, usual: Hint)}
    {@const shown = hint ?? usual}
    <p class="hint" aria-hidden="true"><span>{shown.text}</span>{#if shown.key}<kbd>{shown.key}</kbd>{/if}</p>
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
                <button type="button" class="icon" bind:this={renameButton} aria-label="Rename point" aria-expanded={renaming} data-hint="Change the name of this point." onclick={() => renaming = !renaming}><Icon name="pencil" size={14} /></button>
            {/if}
        </div>
    {/if}
{/snippet}
<Callout label="Map details" {onClose} {takeFocus}>
    {#snippet children(hint)}
        {#if kind === 'add'}
            <header>
                <span class="glyph"><Icon name={hasEndpoints ? 'plus' : 'pin'} size={16} /></span>
                <div><h2>{hasEndpoints ? 'Add a point' : 'Plan from here'}</h2></div>
            </header>
            {#if hasEndpoints && onEndpoint}
                <div class="subs">
                    <button type="button" class="sub" data-hint="The start moves here." onclick={() => onEndpoint('start')}><Icon path={START} size={15} />Start here</button>
                    <button type="button" class="sub" data-hint="The finish moves here." onclick={() => onEndpoint('finish')}><Icon name="flag" size={15} />Finish here</button>
                </div>
            {/if}
            {@render extra?.()}
            <div class="dock">
                {#if hasEndpoints}
                    {@render says(hint, { text: types[0].note, key: types[0].key })}
                    <div class="tiles" style:--n={types.length}>
                        {#each types as type, i (type.value)}
                            {@render tile({ id: type.value, label: type.label, icon: type.icon, note: type.note, key: type.key, go: i === 0, aim: i === 0, run: () => onAddHere(type.value) })}
                        {/each}
                    </div>
                {:else if onEndpoint}
                    {@render says(hint, { text: hasStart ? 'The route ends here.' : 'The route starts here.', key: 'Enter' })}
                    <div class="tiles" style:--n={2}>
                        {@render tile({ ...startTile(false), go: !hasStart, aim: !hasStart })}
                        {@render tile({ ...finishTile(false), go: hasStart, aim: hasStart })}
                    </div>
                {/if}
            </div>
        {:else if kind === 'leg'}
            <header>
                <span class="glyph on-route"><Icon name="route" size={16} /></span>
                <div><h2>This leg</h2></div>
            </header>
            <div class="track" role="radiogroup" aria-label="Leg mode" style:--n={3}>
                {#each legModes as mode, i (mode.value)}
                    <button type="button" class="tile" role="radio" aria-checked={legMode === mode.value} tabindex={legMode === mode.value ? 0 : -1} data-hint={mode.note}
                        onclick={() => onLegMode(mode.value)} onkeydown={(event) => radioKey(event, i, legModes.length, next => onLegMode(legModes[next].value))}>
                        <Icon name={mode.icon} size={21} />{mode.label}
                    </button>
                {/each}
            </div>
            <div class="dock">
                {@render says(hint, { text: 'A new point on this leg.', key: 'Enter' })}
                <div class="tiles" style:--n={canMoveLoopStart(trip) ? 2 : 1}>
                    {@render tile({ id: 'insert', label: 'Add point here', icon: 'plus', note: 'A new point on this leg.', go: true, aim: true, run: onInsert })}
                    {#if canMoveLoopStart(trip)}
                        {@render tile({ id: 'loop', label: 'Start loop here', icon: 'loop', note: 'The start moves here. The stops keep their order.', run: () => onLoopStart?.() })}
                    {/if}
                </div>
            </div>
        {:else if kind === 'dayend'}
            {@const day = days[night - 1]}
            <header>
                <span class="glyph" style:background={dayColor(night)} style:color="#fff">{night}</span>
                <div><h2>Day {night} ends here</h2><p class="meta">For now{day ? ` · ${figures(day)}` : ''}</p></div>
            </header>
            <div class="dock">
                {@render says(hint, { text: 'Towns with a place to sleep near here.', key: 'Enter' })}
                <div class="tiles" style:--n={2}>
                    {@render tile({ id: 'town', label: 'Choose a town', icon: 'camp', note: 'Towns with a place to sleep near here.', go: true, aim: true, run: onChooseNight })}
                    {@render tile({ id: 'pick', label: 'Pick on the map', icon: 'map', note: 'Click the spot of the night on the map.', run: onPick })}
                </div>
            </div>
        {:else if kind === 'place'}
            {#if place}
                <PlaceDetails {place} error={detailsError} />
            {:else}
                <header>
                    <span class="glyph"><Icon name="camp" size={16} /></span>
                    <div><h2>Overnight spot</h2></div>
                </header>
            {/if}
            {#if sleeps}
                {@const pinned = days[sleepDay - 1]?.pinned}
                <div class="night">
                    <span class="stepper">
                        <button type="button" aria-label="Earlier day" disabled={sleepDay <= 1} onclick={() => sleepDay--}><Icon name="chevron" size={13} /></button>
                        <span>Day {sleepDay}</span>
                        <button type="button" aria-label="Later day" disabled={sleepDay >= days.length} onclick={() => sleepDay++}><Icon name="chevron" size={13} /></button>
                    </span>
                    <span class="meta" role="status">
                        {#if sleepDay >= maxRidingDays}A trip can have up to {maxRidingDays} riding days. Choose an earlier day.
                        {:else}{#if pinned}replaces {pinned.label}{:else if sleepDay === days.length}adds a night{/if}{#if preview}{pinned || sleepDay === days.length ? ' · ' : 'would be '}<b>{figures(preview)}</b>{/if}{/if}
                    </span>
                </div>
            {/if}
            {#if placeTiles.length}
                {@const first = placeTiles.find(t => t.id === usual) ?? placeTiles[0]}
                <div class="dock">
                    {@render says(hint, { text: first.note, key: first.key ?? 'Enter' })}
                    <div class="tiles" style:--n={placeTiles.length}>
                        {#each placeTiles as t (t.id)}{@render tile({ ...t, go: t === first, aim: t === first })}{/each}
                    </div>
                </div>
            {/if}
        {:else if kind === 'point' && point}
            {@const current = point.kind === 'detour' ? 'waypoint' : point.kind}
            {@const typed = hasEndpoints && point.kind !== 'start' && point.kind !== 'finish'}
            {#if place}
                <PlaceDetails {place} {title} error={detailsError} />
            {:else}
                <header>
                    <span class="glyph"><Icon name={pointIcons[point.kind] ?? 'flag'} path={point.kind === 'start' ? START : undefined} size={16} /></span>
                    <div>{@render title()}</div>
                </header>
            {/if}
            {#if point.kind === 'finish' && hasEndpoints}
                <div class="subs" style:--n={1}>
                    <button type="button" class="sub" data-hint="The route returns to the start, and this point is the last stop." onclick={onLoop}><Icon name="loop" size={15} />Back to start</button>
                </div>
            {/if}
            <div class="dock">
                {#if typed}{@render says(hint, { text: types.find(type => type.value === current)?.note ?? '' })}{/if}
                <div class="split">
                    {#if typed}
                        <div class="track" role="radiogroup" aria-label="Point type" style:--n={types.length}>
                            {#each types as type, i (type.value)}
                                <button type="button" class="tile" role="radio" aria-checked={current === type.value} tabindex={current === type.value || (i === 0 && !types.some(t => t.value === current)) ? 0 : -1}
                                    data-hint={type.note} data-aim={current === type.value ? '' : undefined}
                                    onclick={() => onKind(type.value)} onkeydown={(event) => radioKey(event, i, types.length, next => onKind(types[next].value))}>
                                    <Icon name={type.icon} size={21} />{type.label}
                                </button>
                            {/each}
                        </div>
                    {:else}{@render says(hint, { text: 'Drag the pin to move this point.' })}{/if}
                    {@render tile({ id: 'remove', label: 'Remove', name: 'Remove point', icon: 'trash', note: 'Removes this point.', key: 'Delete', danger: true, run: onRemove })}
                </div>
            </div>
        {/if}
    {/snippet}
</Callout>

<style>
    .title {
        display: flex;
        align-items: center;
        gap: 4px;
        min-height: 20px;
    }
    .icon {
        flex: none;
        display: grid;
        place-items: center;
        width: 24px;
        height: 24px;
        margin-block: -2px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .icon:hover {
        background: var(--tile);
        color: var(--ink);
    }
    /* The field takes the place of the title, with its text where the title was. */
    .rename {
        flex: 1;
        min-width: 0;
        height: 26px;
        margin: -4px 0 -4px -7px;
        padding: 0 6px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 15px var(--sans);
    }
    /* The day of the night: one click on an arrow for each day, and what the day would be. */
    .night {
        display: flex;
        align-items: center;
        gap: 10px;
        padding: 0 4px;
    }
    .night b {
        color: var(--ink);
        font-weight: 600;
    }
    .stepper {
        display: inline-flex;
        flex: none;
        align-items: center;
        gap: 2px;
        padding: 2px;
        border-radius: 9px;
        background: var(--track);
        font: 600 12px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .stepper span {
        padding: 0 8px;
    }
    .stepper button {
        display: grid;
        place-items: center;
        width: 28px;
        height: 28px;
        border-radius: 7px;
        background: var(--knob);
        box-shadow: inset 0 0 0 1px var(--lit-ring);
    }
    .stepper button:disabled {
        opacity: .35;
    }
    .stepper button:first-child :global(svg) {
        transform: scaleX(-1);
    }
</style>
