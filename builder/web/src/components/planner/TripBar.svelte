<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import PlanTotals from './PlanTotals.svelte';
    import Select from './PlannerSelect.svelte';
    import VersionsMenu from './VersionsMenu.svelte';
    import { onMount, type ComponentProps } from 'svelte';
    import { planPreset, ridingProfiles, servedPresets, type BikeType } from '../../lib/planner/riding-profiles';
    import { planTitle, type Trip } from '../../lib/planner/editor';
    import { regionProfiles, type RoutingLine } from '../../lib/planner/routing';
    import type { Version } from '../../lib/planner/versions';

    let { trip, line, name, totals, versions, openId, canUndo, canRedo, draftSavedAt, draftError, onChange, onUndo, onRedo, onOpen, onSaveNew, onVersions, onCompare, onNew, onLibrary, ready }: {
        trip: Trip;
        line?: RoutingLine;
        name: string;
        /** The totals of the whole plan, or null while it has no route. */
        totals: ComponentProps<typeof PlanTotals> | null;
        versions: Version[];
        openId?: string;
        ready: boolean;
        onLibrary: () => void;
        canUndo: boolean;
        canRedo: boolean;
        draftSavedAt: number | null;
        draftError: string;
        onChange: (change: Partial<Trip>, description: string) => void;
        onNew: () => void;
        onUndo: () => void;
        onRedo: () => void;
        onOpen: (id: string, name: string) => void;
        onSaveNew: (name: string) => Promise<void>;
        onVersions: (versions: Version[]) => Promise<void>;
        onCompare: () => void;
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    const title = $derived(name || planTitle(trip));
    // The profiles of the routing region; until they arrive, every activity and preset shows.
    let profiles = $state<string[]>();
    // An activity that the region does not serve stays listed while the plan uses it.
    const activities = $derived(Object.entries(ridingProfiles).filter(([value]) => value === bike || servedPresets(value as BikeType, profiles).length));

    onMount(() => { regionProfiles().then(found => profiles = found, () => {}); });
</script>

<div class="trip-bar" inert={!ready}>
    <div class="trip-name">
        <h1>{title}</h1>
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
        {#if totals}<PlanTotals {...totals} />{/if}
    </div>
    <div class="actions">
        <button type="button" class="planner-action quiet" disabled={!ready || !trip.points.length} onclick={onNew}>New {trip.mode === 'route' ? 'route' : 'trip'}</button>
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
        <button type="button" class="planner-action" disabled={!ready} onclick={onLibrary}>My plans</button>
        <VersionsMenu {trip} {line} {versions} {openId} {draftSavedAt} {draftError} {onOpen} {onSaveNew} onChange={onVersions} {onCompare} />
    </div>
</div>

<style>
    .trip-bar {
        display: grid;
        grid-template-columns: calc(var(--side-width) - 16px) minmax(0, 1fr) auto;
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
        margin: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font: 700 17px var(--sans);
    }
    /* The totals go to a row of their own where they do not fit beside Activity and Preset. */
    .ride {
        display: flex;
        align-items: center;
        flex-wrap: wrap;
        gap: 4px 16px;
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
    /* Narrower, the middle column beside the panel-wide name has no room for Activity and Preset. */
    @media (max-width: 1200px) {
        .trip-bar { grid-template-columns: minmax(0, 1fr) auto; gap: 8px 16px; }
        .ride { grid-column: 1 / -1; grid-row: 2; }
        .actions { grid-column: 2; grid-row: 1; }
    }
    @media (max-width: 760px) {
        .trip-bar { grid-template-columns: minmax(0, 1fr); }
        .trip-name h1 { flex: 1; }
        .actions { grid-column: 1; grid-row: 3; justify-content: flex-end; }
    }
    .actions > :global(.versions) {
        margin-left: 8px;
    }
</style>
