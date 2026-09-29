<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import Select from './PlannerSelect.svelte';
    import VersionsMenu from './VersionsMenu.svelte';
    import { ridingProfiles, type BikeType } from '../../lib/planner/riding-profiles';
    import type { Trip } from '../../lib/planner/editor';
    import type { Version } from '../../lib/planner/versions';

    let { trip, canUndo, canRedo, draftSavedAt, draftError, onChange, onUndo, onRedo, onRestore, onSaved, onNew }: {
        trip: Trip;
        canUndo: boolean;
        canRedo: boolean;
        draftSavedAt: number | null;
        draftError: string;
        onChange: (change: Partial<Trip>, description: string) => void;
        onNew: () => void;
        onUndo: () => void;
        onRedo: () => void;
        onRestore: (trip: Trip, name: string) => void;
        onSaved: (version: Version) => void;
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    const start = $derived(trip.points.find(p => p.kind === 'start'));
    const finish = $derived(trip.points.find(p => p.kind === 'finish'));
    const title = $derived(start && finish ? `${start.label} → ${finish.label}` : start ? `From ${start.label}` : finish ? `To ${finish.label}` : 'New plan');
</script>

<div class="trip-bar">
    <div class="trip-name">
        <h1>{title}</h1>
        <Select label="Plan type" value={trip.mode ?? 'trip'} options={[{ value: 'trip', label: 'Multi-day trip' }, { value: 'route', label: 'Single route' }]}
            onChange={(mode) => onChange({ mode: mode as Trip['mode'] }, mode === 'route' ? 'Single route' : 'Multi-day trip')} />
    </div>
    <div class="ride">
        <div class="preference"><span>Bike</span>
            <Select label="Bike" value={bike} options={Object.entries(ridingProfiles).map(([value, profile]) => ({ value, label: profile.label }))} onChange={(value) => {
                const next = value as BikeType;
                onChange({ bike: next, preset: ridingProfiles[next].presets[0] }, 'Bike profile changed');
            }} />
        </div>
        <div class="preference"><span>Preset</span>
            <Select label="Preset" value={trip.preset ?? 'Balanced'} options={ridingProfiles[bike].presets.map(value => ({ value, label: value }))}
                onChange={(preset) => onChange({ preset }, 'Route preference changed')} />
        </div>
    </div>
    <div class="actions">
        <button type="button" class="planner-action quiet" disabled={!trip.points.length} onclick={onNew}>New {trip.mode === 'route' ? 'route' : 'trip'}</button>
        <button type="button" class="icon" disabled={!canUndo} onclick={onUndo} aria-label="Undo" title="Undo"><Icon name="undo" /></button>
        <button type="button" class="icon" disabled={!canRedo} onclick={onRedo} aria-label="Redo" title="Redo"><Icon name="redo" /></button>
        <VersionsMenu {trip} {draftSavedAt} {draftError} {onRestore} {onSaved} />
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
        gap: 4px;
    }
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
    .actions > :global(.versions) {
        margin-left: 8px;
    }
</style>
