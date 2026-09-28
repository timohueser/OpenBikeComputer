<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import VersionsMenu from './VersionsMenu.svelte';
    import { ridingProfiles, type BikeType } from '../../lib/planner/riding-profiles';
    import type { Trip } from '../../lib/planner/editor';
    import type { Version } from '../../lib/planner/versions';

    let { trip, canUndo, canRedo, draftSavedAt, draftError, onChange, onUndo, onRedo, onRestore, onSaved }: {
        trip: Trip;
        canUndo: boolean;
        canRedo: boolean;
        draftSavedAt: number | null;
        draftError: string;
        onChange: (change: Partial<Trip>, description: string) => void;
        onUndo: () => void;
        onRedo: () => void;
        onRestore: (trip: Trip, name: string) => void;
        onSaved: (version: Version) => void;
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    const title = $derived(`${trip.points.find(p => p.kind === 'start')?.label} → ${trip.points.find(p => p.kind === 'finish')?.label}`);
</script>

<div class="trip-bar">
    <div class="trip-name">
        <h1>{title}</h1>
        <select class="quiet" aria-label="Plan type" value={trip.mode ?? 'trip'}
            onchange={(event) => onChange({ mode: event.currentTarget.value as Trip['mode'] }, event.currentTarget.value === 'route' ? 'Single route' : 'Multi-day trip')}>
            <option value="trip">Multi-day trip</option>
            <option value="route">Single route</option>
        </select>
    </div>
    <div class="ride">
        <label>Bike
            <select value={bike} onchange={(event) => {
                const next = event.currentTarget.value as BikeType;
                onChange({ bike: next, preset: ridingProfiles[next].presets[0] }, 'Bike preference saved · routing is mocked');
            }}>
                {#each Object.entries(ridingProfiles) as [id, profile]}<option value={id}>{profile.label}</option>{/each}
            </select>
        </label>
        <label>Preset
            <select value={trip.preset ?? 'Balanced'} onchange={(event) => onChange({ preset: event.currentTarget.value }, 'Preset saved · routing is mocked')}>
                {#each ridingProfiles[bike].presets as preset}<option>{preset}</option>{/each}
            </select>
        </label>
    </div>
    <div class="actions">
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
        align-items: baseline;
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
    select {
        height: 30px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
        cursor: pointer;
    }
    select.quiet {
        flex: none;
        height: 26px;
        padding: 0 4px;
        border-color: transparent;
        color: var(--ink-soft);
        font-weight: 400;
    }
    select.quiet:hover {
        border-color: var(--line-strong);
    }
    .ride {
        display: flex;
        align-items: center;
        gap: 16px;
    }
    label {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .ride select {
        max-width: 170px;
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
    .actions > :global(.versions) {
        margin-left: 8px;
    }
</style>
