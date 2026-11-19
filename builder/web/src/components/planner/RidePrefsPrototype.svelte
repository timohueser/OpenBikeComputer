<!-- PROTOTYPE (prototype/trip-bar): one control for the plan type, the activity, the preset and the pace. `inline` opens
     the choices in the page under the button; otherwise they open in a panel under the button. -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Select from './PlannerSelect.svelte';
    import { onMount } from 'svelte';
    import { paces, planPace, planPreset, ridingProfiles, servedPresets, type BikeType } from '../../lib/planner/riding-profiles';
    import type { Trip } from '../../lib/planner/editor';
    import { regionProfiles } from '../../lib/planner/routing';

    let { trip, onChange, inline = false, compact = false, noType = false }: {
        trip: Trip;
        onChange: (change: Partial<Trip>, description: string) => void;
        inline?: boolean;
        /** Only the icon and the activity on the button. */
        compact?: boolean;
        /** PROTOTYPE: the type has another home. */
        noType?: boolean;
    } = $props();

    const bike = $derived(trip.bike ?? 'touring');
    let profiles = $state<string[]>();
    const activities = $derived(Object.entries(ridingProfiles).filter(([value]) => value === bike || servedPresets(value as BikeType, profiles).length));
    const preset = $derived(planPreset(bike, trip.preset));
    const pace = $derived(paces[planPace(trip)].label);
    const type = $derived(trip.mode === 'route' ? 'Route' : 'Multi-day trip');
    const summary = $derived(inline ? [ridingProfiles[bike].label, preset, `${pace} pace`] : compact ? [ridingProfiles[bike].label] : [ridingProfiles[bike].label, preset]);

    let open = $state(false);
    let root: HTMLDivElement;
    let opener: HTMLButtonElement;

    onMount(() => { regionProfiles().then(found => profiles = found, () => {}); });

    function key(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open || inline) return;
        event.stopPropagation();
        open = false;
        opener.focus();
    }
</script>

<svelte:window onpointerdown={event => { if (open && !inline && !root.contains(event.target as Node)) open = false; }} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="prefs" class:inline bind:this={root} onkeydown={key}>
    <button type="button" class="opener" class:chosen={open} bind:this={opener} aria-expanded={open}
        aria-label={`Route preferences: ${type}, ${ridingProfiles[bike].label}, ${preset}, ${pace} pace`} onclick={() => open = !open}>
        <Icon name={ridingProfiles[bike].icon} size={20} />
        <span class="text">{summary.join(' · ')}</span>
        <Icon name="down" size={14} />
    </button>
    {#if open}
        <div class="panel" role={inline ? 'group' : 'dialog'} aria-label="Route preferences">
            {#if !noType}
            <span>Plan</span>
            <Select label="Plan type" value={trip.mode ?? 'trip'} options={[{ value: 'trip', label: 'Multi-day trip' }, { value: 'route', label: 'Route' }]}
                onChange={(mode) => onChange({ mode: mode as Trip['mode'] }, mode === 'route' ? 'Route' : 'Multi-day trip')} />
            {/if}
            <span>Activity</span>
            <Select label="Activity" value={bike} options={activities.map(([value, profile]) => ({ value, label: profile.label, icon: profile.icon }))} onChange={(value) => {
                const next = value as BikeType;
                onChange({ bike: next, preset: servedPresets(next, profiles)[0] }, 'Bike profile changed');
            }} />
            <span>Preset</span>
            <Select label="Preset" value={preset} options={servedPresets(bike, profiles).map(value => ({ value, label: value, icon: value === 'Less climbing' ? 'less-climbing' : 'sliders' }))}
                onChange={(next) => onChange({ preset: next }, 'Route preference changed')} />
            <span>Pace</span>
            <Select label="Pace" value={planPace(trip)} options={Object.entries(paces).map(([value, { label }]) => ({ value, label }))}
                onChange={(value) => onChange({ pace: value as Trip['pace'] }, 'Pace changed')} />
        </div>
    {/if}
</div>

<style>
    .prefs { position: relative; min-width: 0; }
    .opener {
        display: inline-flex; align-items: center; gap: 8px; max-width: 100%; min-height: 32px; padding: 5px 9px;
        border: 1px solid var(--line-strong); border-radius: 6px; background: var(--panel);
        color: var(--ink); font: 600 13px var(--sans); cursor: pointer;
    }
    .opener:hover, .opener.chosen { border-color: var(--ink-faint); background: var(--parchment-2); }
    .opener > :global(svg) { flex: none; }
    .opener > :global(svg:last-child) { color: var(--ink-soft); }
    .text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .panel {
        position: absolute; top: calc(100% + 6px); left: 0; z-index: 45;
        display: grid; grid-template-columns: auto minmax(0, 1fr); align-items: center; gap: 8px 12px;
        width: 280px; padding: 12px; border: 1px solid var(--line-strong); border-radius: 8px;
        background: var(--panel); box-shadow: var(--planner-shadow); font-size: 13px; color: var(--ink-soft);
    }
    .panel :global(.trigger) { width: 100%; }
    /* In the side panel: a quiet line that opens the choices in the page. */
    .inline .opener { width: 100%; border: 0; border-radius: 0; padding: 8px 16px; background: none; color: var(--ink-soft); font-weight: 400; }
    .inline .opener:hover, .inline .opener.chosen { background: var(--parchment-2); color: var(--ink); }
    .inline .opener > :global(svg:first-child) { color: var(--ink); }
    .inline .text { flex: 1; text-align: left; }
    .inline .panel { position: static; width: auto; padding: 4px 16px 12px; border: 0; border-radius: 0; box-shadow: none; background: none; }
</style>
