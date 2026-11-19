<!-- One line with the activity, the preset and the pace of the route; it opens the three choices in the page. -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Select from './PlannerSelect.svelte';
    import { onMount } from 'svelte';
    import { paces, planPace, planPreset, ridingProfiles, servedPresets, type BikeType } from '../../lib/planner/riding-profiles';
    import type { Trip } from '../../lib/planner/editor';
    import { regionProfiles } from '../../lib/planner/routing';

    let { trip, onChange }: { trip: Trip; onChange: (change: Partial<Trip>, description: string) => void } = $props();

    const id = $props.id();
    const bike = $derived(trip.bike ?? 'touring');
    // The profiles of the routing region; until they arrive, every activity and preset shows.
    let profiles = $state<string[]>();
    // An activity that the region does not serve stays listed while the plan uses it.
    const activities = $derived(Object.entries(ridingProfiles).filter(([value]) => value === bike || servedPresets(value as BikeType, profiles).length));
    const summary = $derived(`${ridingProfiles[bike].label} · ${planPreset(bike, trip.preset)} · ${paces[planPace(trip)].label} pace`);
    let open = $state(false);
    let button: HTMLButtonElement;

    onMount(() => { regionProfiles().then(found => profiles = found, () => {}); });

    export function focus() { button.focus(); }

    // The planner clears the map selection and closes a day on Escape, so this Escape stops here.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !open) return;
        event.preventDefault();
        event.stopPropagation();
        open = false;
        button.focus();
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="preferences" onkeydown={escape}>
    <button type="button" class="line" bind:this={button} title={summary} aria-expanded={open} aria-controls={id} onclick={() => open = !open}>
        <Icon name={ridingProfiles[bike].icon} size={20} />
        <span class="text"><span class="sr-only">Route preferences:</span> {summary}</span>
        <Icon name="down" size={14} />
    </button>
    {#if open}
        <div class="choices" role="group" aria-label="Choose route preferences" {id}>
            <span>Activity</span>
            <Select label="Activity" value={bike} options={activities.map(([value, profile]) => ({ value, label: profile.label, icon: profile.icon }))} onChange={(value) => {
                const next = value as BikeType;
                onChange({ bike: next, preset: servedPresets(next, profiles)[0] }, 'Bike profile changed');
            }} />
            <span>Preset</span>
            <Select label="Preset" value={planPreset(bike, trip.preset)} options={servedPresets(bike, profiles).map(value => ({ value, label: value, icon: value === 'Less climbing' ? 'less-climbing' : 'sliders' }))}
                onChange={(preset) => onChange({ preset }, 'Route preference changed')} />
            <span>Pace</span>
            <Select label="Pace" value={planPace(trip)} options={Object.entries(paces).map(([value, { label }]) => ({ value, label }))}
                onChange={(value) => onChange({ pace: value as Trip['pace'] }, 'Pace changed')} />
        </div>
    {/if}
</div>

<style>
    .line {
        display: flex;
        align-items: center;
        gap: 8px;
        width: 100%;
        min-height: 40px;
        padding: 8px 16px;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font: 13px var(--sans);
        cursor: pointer;
    }
    .line:hover, .line[aria-expanded="true"] { background: var(--parchment-2); color: var(--ink); }
    .line:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
    .line > :global(svg) { flex: none; }
    .line > :global(svg:first-child) { color: var(--ink); }
    .text {
        flex: 1;
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        text-align: left;
    }
    .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .choices {
        display: grid;
        grid-template-columns: auto minmax(0, 1fr);
        align-items: center;
        gap: 8px 12px;
        padding: 8px 16px 12px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .choices :global(.trigger) { width: 100%; }
</style>
