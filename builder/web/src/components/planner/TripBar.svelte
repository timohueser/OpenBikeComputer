<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Menu from '../../lib/ui/Menu.svelte';
    import Select from './PlannerSelect.svelte';
    import RidePrefs from './RidePrefsPrototype.svelte';
    import { onMount } from 'svelte';
    import { paces, planPace, planPreset, ridingProfiles, servedPresets, type BikeType } from '../../lib/planner/riding-profiles';
    import { planTitle, type Trip } from '../../lib/planner/editor';
    import { regionProfiles } from '../../lib/planner/routing';

    let { trip, name, stored, changed, canSave, canUndo, canRedo, onChange, onUndo, onRedo, onNew, onLibrary, onSave, onRename, more, ready }: {
        trip: Trip;
        name: string;
        /** The route has a record in the library: its title renames it, and with no changes it is "Saved". */
        stored: boolean;
        changed: boolean;
        canSave: boolean;
        ready: boolean;
        onLibrary: () => void;
        canUndo: boolean;
        canRedo: boolean;
        onChange: (change: Partial<Trip>, description: string) => void;
        onNew: () => void;
        onUndo: () => void;
        onRedo: () => void;
        onSave: () => void;
        onRename: () => void;
        /** The items of the "More" menu. */
        more: { label: string; disabled?: boolean; rule?: boolean; run: () => void }[];
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    const title = $derived(name || planTitle(trip));
    const saved = $derived(stored && !changed);
    // The profiles of the routing region; until they arrive, every activity and preset shows.
    let profiles = $state<string[]>();
    // An activity that the region does not serve stays listed while the plan uses it.
    const activities = $derived(Object.entries(ridingProfiles).filter(([value]) => value === bike || servedPresets(value as BikeType, profiles).length));

    onMount(() => { regionProfiles().then(found => profiles = found, () => {}); });

    // PROTOTYPE: `?bar=A|B|C` selects a direction for the trip bar.
    const bar = new URLSearchParams(location.search).get('bar');
    const newItem = $derived({ label: `New ${trip.mode === 'route' ? 'route' : 'trip'}`, disabled: !ready || !trip.points.length, run: onNew });
    const moreItems = $derived([newItem, { ...more[0], rule: true }, ...more.slice(1)]);
    const titleItems = $derived([
        { label: 'Rename…', disabled: !stored, run: onRename },
        { ...newItem, rule: true },
        { label: 'Open route…', disabled: !ready, run: onLibrary },
        { ...more[0], rule: true }, ...more.slice(1),
    ]);
    let narrow = $state(false);
    onMount(() => {
        const query = matchMedia('(max-width: 600px)');
        narrow = query.matches;
        query.onchange = () => narrow = query.matches;
    });
</script>

{#if bar}
<div class="trip-bar proto" data-bar={bar} inert={!ready}>
    {#if bar === 'B'}
        <div class="t title-menu"><Menu label={title} items={titleItems} left buttonClass="title-button" /></div>
    {:else}
        <h1 class="t">{#if stored}<button type="button" class="rename" {title} aria-haspopup="dialog" onclick={onRename}>{title}</button>{:else}<span {title}>{title}</span>{/if}</h1>
    {/if}
    {#if bar !== 'C'}<div class="p"><RidePrefs {trip} {onChange} compact={narrow} /></div>{/if}
    <div class="h">
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
    </div>
    {#if bar === 'A'}<button type="button" class="l planner-action" disabled={!ready} onclick={onLibrary}>My routes</button>{/if}
    <div class="s save">
        <button type="button" class={saved ? 'saved' : 'planner-action primary'} aria-disabled={!ready || !canSave}
            onclick={() => { if (ready && canSave) onSave(); }}>{#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}</button>
        {#if bar !== 'B'}<Menu label="More" items={moreItems} buttonClass="planner-action quiet" />{/if}
    </div>
</div>
{:else}

<div class="trip-bar" inert={!ready}>
    <div class="trip-name">
        <h1>{#if stored}<button type="button" class="rename" {title} aria-haspopup="dialog" onclick={onRename}>{title}</button>{:else}<span {title}>{title}</span>{/if}</h1>
        <Select label="Plan type" value={trip.mode ?? 'trip'} options={[{ value: 'trip', label: 'Multi-day trip' }, { value: 'route', label: 'Route' }]}
            onChange={(mode) => onChange({ mode: mode as Trip['mode'] }, mode === 'route' ? 'Route' : 'Multi-day trip')} />
    </div>
    <div class="ride">
        <div class="preference"><span>Activity</span>
            <Select label="Activity" value={bike} options={activities.map(([value, profile]) => ({ value, label: profile.label, icon: profile.icon }))} onChange={(value) => {
                const next = value as BikeType;
                onChange({ bike: next, preset: servedPresets(next, profiles)[0] }, 'Bike profile changed');
            }} />
        </div>
        <div class="preference"><span>Preset</span>
            <Select label="Preset" value={planPreset(bike, trip.preset)} options={servedPresets(bike, profiles).map(value => ({ value, label: value, icon: value === 'Less climbing' ? 'less-climbing' : 'sliders' }))}
                onChange={(preset) => onChange({ preset }, 'Route preference changed')} />
        </div>
        <div class="preference"><span>Pace</span>
            <Select label="Pace" value={planPace(trip)} options={Object.entries(paces).map(([value, { label }]) => ({ value, label }))}
                onChange={(value) => onChange({ pace: value as Trip['pace'] }, 'Pace changed')} />
        </div>
    </div>
    <div class="actions">
        <button type="button" class="planner-action quiet" disabled={!ready || !trip.points.length} onclick={onNew}>New {trip.mode === 'route' ? 'route' : 'trip'}</button>
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
        <button type="button" class="planner-action" disabled={!ready} onclick={onLibrary}>My plans</button>
        <div class="save">
            <!-- One button that stays in the page, so it keeps the focus after a save. -->
            <button type="button" class={saved ? 'saved' : 'planner-action primary'} aria-disabled={!ready || !canSave}
                onclick={() => { if (ready && canSave) onSave(); }}>{#if saved}<Icon name="check" size={15} />Saved{:else}Save{/if}</button>
            <Menu label="More" items={more} buttonClass="planner-action quiet" />
        </div>
    </div>
</div>
{/if}

<style>
    .trip-bar {
        display: grid;
        grid-template-columns: minmax(calc(var(--side-width) - 16px), max-content) minmax(min-content, 1fr) auto;
        align-items: center;
        gap: 16px;
        min-height: 56px;
        flex: none;
        padding: 8px 16px;
        border-bottom: 1px solid var(--line);
        background: var(--panel);
    }
    .trip-name {
        display: flex;
        align-items: center;
        gap: 12px;
        min-width: 0;
    }
    h1 {
        display: flex;
        min-width: 0;
        margin: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font: 700 17px var(--sans);
    }
    h1 > * {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .rename {
        margin: 0 -6px;
        padding: 2px 6px;
        white-space: nowrap;
        border: 0;
        border-radius: 6px;
        background: none;
        color: inherit;
        font: inherit;
        text-align: left;
    }
    .rename:hover { background: var(--parchment-2); }
    .save {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 8px;
    }
    .saved {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 8px;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font: inherit;
        font-weight: 600;
        cursor: default;
    }
    .primary[aria-disabled="true"] {
        opacity: 0.45;
        cursor: default;
        pointer-events: none;
    }
    .ride {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 8px 16px;
    }
    .preference {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .actions {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 4px;
    }
    .actions :global(button) { white-space: nowrap; }
    .icon {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
    }
    .icon:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    @media (max-width: 1050px) {
        .trip-bar { grid-template-columns: minmax(0, 1fr) auto; gap: 8px 16px; }
        .ride { grid-column: 1; grid-row: 2; }
        .actions { grid-column: 2; grid-row: 1 / span 2; }
    }
    @media (max-width: 760px) {
        .trip-bar { grid-template-columns: minmax(0, 1fr); }
        .trip-name h1 { flex: 1; }
        .actions { grid-column: 1; grid-row: 3; justify-content: flex-end; }
    }
    /* PROTOTYPE directions. */
    .proto { display: flex; align-items: center; gap: 12px; }
    .proto .t { flex: 0 1 auto; min-width: 0; }
    .proto .p { flex: 0 0 auto; min-width: 0; }
    .proto .h { display: flex; gap: 2px; margin-left: auto; flex: none; }
    .proto .l, .proto .s { flex: none; }
    .proto .save { margin-left: 4px; }
    .proto :global(button) { white-space: nowrap; }
    .title-menu { display: flex; margin-left: -6px; }
    .title-menu :global(.menu) { min-width: 0; }
    .title-menu :global(.title-button) {
        max-width: 100%; margin: 0; padding: 4px 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
        border: 0; border-radius: 6px; background: none; color: var(--ink); font: 700 17px var(--sans); text-align: left; cursor: pointer;
    }
    .title-menu :global(.title-button:hover), .title-menu :global(.title-button[aria-expanded="true"]) { background: var(--parchment-2); }
    @media (max-width: 600px) {
        .proto[data-bar="A"], .proto[data-bar="B"] {
            display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 8px; align-items: center;
            grid-template-areas: "t t s" "p h l";
        }
        .proto[data-bar="B"] { grid-template-areas: "t t s" "p p h"; }
        .proto .t { grid-area: t; }
        .proto .p { grid-area: p; justify-self: start; }
        .proto .h { grid-area: h; margin: 0; }
        .proto .l { grid-area: l; justify-self: end; }
        .proto .s { grid-area: s; margin: 0; }
        .proto[data-bar="C"] { gap: 6px; }
    }
</style>
