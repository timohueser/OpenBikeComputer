<!-- One line with the activity, the preset and the pace of the route, and the kinds of place to sleep of a trip when they
     are not all on; it opens the choices in the page. -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Select from './PlannerSelect.svelte';
    import { onMount } from 'svelte';
    import { paces, planPace, planPreset, ridingProfiles, servedPresets, type BikeType } from '../../lib/planner/riding-profiles';
    import { allStay, multiDay, type Stay, type Trip } from '../../lib/planner/editor';
    import { regionProfiles } from '../../lib/planner/routing';
    import { stayIcon, stayOptions } from '../../lib/planner/search/presentation';

    let { trip, onChange }: { trip: Trip; onChange: (change: Partial<Trip>, description: string) => void } = $props();

    const id = $props.id();
    const bike = $derived(trip.bike ?? 'touring');
    // The profiles of the routing region; until they arrive, every activity and preset shows.
    let profiles = $state<string[]>();
    // An activity that the region does not serve stays listed while the plan uses it.
    const activities = $derived(Object.entries(ridingProfiles).filter(([value]) => value === bike || servedPresets(value as BikeType, profiles).length));
    // The suggested nights of a trip look for these kinds; a route of one day has no night.
    const sleeps = $derived(multiDay(trip));
    const stay = $derived(trip.stay ?? allStay);
    const stayLabels = (kinds: Stay[]) => stayOptions.filter(option => kinds.includes(option.kind)).map(option => option.label).join(', ');
    const summary = $derived(`${ridingProfiles[bike].label} · ${planPreset(bike, trip.preset)} · ${paces[planPace(trip)].label} pace${sleeps && trip.stay ? ` · ${stayLabels(stay)}` : ''}`);
    let open = $state(false);
    let button: HTMLButtonElement;

    onMount(() => { regionProfiles().then(found => profiles = found, () => {}); });

    export function focus() { button.focus(); }

    // All kinds on is any place, which the plan stores as no choice.
    function toggleStay(kind: Stay) {
        const next = allStay.filter(other => (other === kind) !== stay.includes(other));
        onChange({ stay: next.length === allStay.length ? undefined : next }, `Sleeping in ${stayLabels(next).toLowerCase()}`);
    }

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
            {#if sleeps}
                <span class="stay-label">Sleep in</span>
                <div class="stay" role="group" aria-label="Sleep in">
                    {#each stayOptions as { kind, label } (kind)}
                        {@const on = stay.includes(kind)}
                        <!-- One kind stays on. -->
                        <button type="button" aria-pressed={on} disabled={on && stay.length === 1} onclick={() => toggleStay(kind)}><Icon name={stayIcon(kind)} size={14} />{label}</button>
                    {/each}
                </div>
            {/if}
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
    /* The kinds of place to sleep make the line long: it wraps, so a narrow side panel hides none of them. */
    .text {
        flex: 1;
        min-width: 0;
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
    /* Beside the first line of the chips, which wrap. */
    .stay-label { align-self: start; line-height: 32px; }
    .stay { display: flex; flex-wrap: wrap; gap: 6px; }
    .stay button {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 11px 0 9px;
        border: 1px solid var(--line-strong);
        border-radius: 999px;
        background: none;
        color: var(--ink-soft);
        font: 600 13px var(--sans);
        cursor: pointer;
    }
    .stay button[aria-pressed="true"] { border-color: var(--ink); background: var(--ink); color: var(--panel); }
    .stay button:disabled { cursor: default; }
    .stay button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
</style>
