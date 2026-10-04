<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import Select from './PlannerSelect.svelte';
    import VersionsMenu from './VersionsMenu.svelte';
    import { ridingProfiles, type BikeType } from '../../lib/planner/riding-profiles';
    import type { Trip } from '../../lib/planner/editor';
    import { planTitle } from '../../lib/planner/versions';
    import type { Version } from '../../lib/planner/versions';

    let { trip, name, versions, canUndo, canRedo, draftSavedAt, draftError, onChange, onUndo, onRedo, onRestore, onVersions, onNew, onLibrary, ready }: {
        trip: Trip;
        name: string;
        versions: Version[];
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
        onRestore: (trip: Trip, name: string) => void;
        onVersions: (versions: Version[]) => Promise<void>;
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    const title = $derived(name || planTitle(trip));
</script>

<div class="trip-bar" inert={!ready}>
    <div class="trip-name">
        <h1>{title}</h1>
        <Select label="Plan type" value={trip.mode ?? 'trip'} options={[{ value: 'trip', label: 'Multi-day trip' }, { value: 'route', label: 'Route' }]}
            onChange={(mode) => onChange({ mode: mode as Trip['mode'] }, mode === 'route' ? 'Route' : 'Multi-day trip')} />
    </div>
    <div class="ride">
        <div class="preference"><span>Activity</span>
            <Select label="Activity" value={bike} options={Object.entries(ridingProfiles).map(([value, profile]) => ({ value, label: profile.label, icon: profile.icon }))} onChange={(value) => {
                const next = value as BikeType;
                onChange({ bike: next, preset: ridingProfiles[next].presets[0] }, 'Bike profile changed');
            }} />
        </div>
        <div class="preference"><span>Preset</span>
            <Select label="Preset" value={trip.preset ?? 'Balanced'} options={ridingProfiles[bike].presets.map(value => ({ value, label: value, icon: value === 'Less climbing' ? 'less-climbing' : value === 'Shorter' ? 'arrow' : 'sliders' }))}
                onChange={(preset) => onChange({ preset }, 'Route preference changed')} />
        </div>
    </div>
    <div class="actions">
        <button type="button" class="planner-action quiet" disabled={!ready || !trip.points.length} onclick={onNew}>New {trip.mode === 'route' ? 'route' : 'trip'}</button>
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
        <button type="button" class="planner-action" disabled={!ready} onclick={onLibrary}>My plans</button>
        <VersionsMenu {trip} {versions} {draftSavedAt} {draftError} {onRestore} onChange={onVersions} />
    </div>
</div>

<style>
    .trip-bar {
        display: grid;
        grid-template-columns: calc(var(--side-width) - 16px) minmax(0, 1fr) auto;
        align-items: center;
        gap: 16px;
        height: 56px;
        flex: none;
        padding: 0 16px;
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
    .ride {
        display: flex;
        align-items: center;
        gap: 16px;
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
        .trip-bar { grid-template-columns: minmax(0, 1fr) auto; height: auto; min-height: 56px; padding-block: 8px; gap: 8px 16px; }
        .ride { grid-column: 1; grid-row: 2; }
        .actions { grid-column: 2; grid-row: 1 / span 2; }
    }
    @media (max-width: 700px) {
        .trip-bar { grid-template-columns: minmax(0, 1fr); }
        .trip-name h1 { flex: 1; }
        .ride { flex-wrap: wrap; }
        .actions { grid-column: 1; grid-row: 3; justify-content: flex-end; }
    }
    .actions > :global(.versions) {
        margin-left: 8px;
    }
</style>
