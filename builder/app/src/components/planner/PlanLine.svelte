<script lang="ts">
    import { tick, untrack } from 'svelte';
    import type { Trip } from '../../lib/planner/editor';

    let { trip, dayCount, editing = $bindable(false), onApply }: {
        trip: Trip;
        /** Calendar days, rest days included. */
        dayCount: number;
        editing?: boolean;
        onApply: (budget: Trip['budget'], target: number, limit: number, climb: number) => void;
    } = $props();

    let budget = $state<Trip['budget']>('days');
    let target = $state(3);
    let limit = $state(50);
    let climb = $state(0);
    let form = $state<HTMLFormElement>();

    const summary = $derived([
        `${dayCount} ${dayCount === 1 ? 'day' : 'days'}`,
        ...(trip.limit > 0 ? [`≤ ${trip.limit} km/day`] : []),
        ...(trip.climbTarget ? [`≤ ${trip.climbTarget} m/day`] : []),
    ].join(' · '));
    const valid = $derived(Number.isFinite(target) && target >= 1 && Number.isFinite(limit) && limit >= 0 && Number.isFinite(climb) && climb >= 0);

    $effect(() => {
        if (!editing) return;
        // The form starts from the trip once per opening; later trip edits must not reset what the rider typed.
        untrack(() => {
            budget = trip.budget;
            target = trip.target;
            limit = trip.limit;
            climb = trip.climbTarget ?? 0;
        });
        tick().then(() => form?.querySelector('select')?.focus());
    });

    function changeBudget() {
        target = budget === 'days' ? dayCount : budget === 'distance' ? 50 : 4;
    }

    function apply() {
        if (!valid) return;
        onApply(budget, target, limit, climb);
        editing = false;
    }
</script>

{#if trip.mode === 'route'}
    <p class="plan-line">Single route</p>
{:else if !editing}
    <p class="plan-line">
        <span>{summary}</span>
        <button type="button" class="link" onclick={() => editing = true}>Edit</button>
    </p>
{:else}
    <!-- Escape bubbles from the fields; the form closes the editor in place. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form class="plan-editor" bind:this={form} aria-label="Day plan"
        onsubmit={(event) => { event.preventDefault(); apply(); }}
        onkeydown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); editing = false; } }}>
        <label class="wide">
            <span>Plan by</span>
            <span class="pair">
                <select bind:value={budget} onchange={changeBudget}>
                    <option value="days">Days available</option>
                    <option value="distance">Distance per day</option>
                    <option value="hours">Riding hours per day</option>
                </select>
                <span class="unit-field">
                    <input type="number" min="1" max={budget === 'days' ? 14 : undefined} required aria-label={budget === 'days' ? 'Number of days' : budget === 'distance' ? 'Kilometres per day' : 'Riding hours per day'} bind:value={target} />
                    <small>{budget === 'days' ? 'days' : budget === 'distance' ? 'km' : 'h'}</small>
                </span>
            </span>
        </label>
        <label>
            <span>Distance target</span>
            <span class="unit-field"><input type="number" min="0" required bind:value={limit} /><small>km</small></span>
        </label>
        <label>
            <span>Climb target</span>
            <span class="unit-field"><input type="number" min="0" step="50" required bind:value={climb} /><small>m</small></span>
        </label>
        <p class="help">0 means no target.</p>
        <div class="buttons">
            <button type="button" class="quiet" onclick={() => editing = false}>Cancel</button>
            <button type="submit" class="primary" disabled={!valid}>Apply</button>
        </div>
    </form>
{/if}

<style>
    .plan-line {
        display: flex;
        align-items: baseline;
        gap: 12px;
        margin: 0;
        padding: 0 16px 16px;
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .plan-line span {
        flex: 1;
    }
    button {
        font: inherit;
        cursor: pointer;
    }
    .link {
        padding: 0;
        border: 0;
        background: none;
        color: var(--forest);
        font-weight: 600;
        text-decoration: underline;
        text-underline-offset: 3px;
    }
    .plan-editor {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 12px;
        margin: 0 16px 16px;
        padding: 16px;
        border-radius: 8px;
        background: var(--parchment-2);
    }
    label {
        display: flex;
        flex-direction: column;
        gap: 4px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .wide {
        grid-column: 1 / -1;
    }
    .pair {
        display: flex;
        gap: 8px;
    }
    .pair select {
        flex: 1;
        min-width: 0;
    }
    select,
    input {
        height: 32px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .unit-field {
        position: relative;
        display: flex;
    }
    .unit-field input {
        width: 100%;
        min-width: 0;
        padding-right: 36px;
    }
    .pair .unit-field {
        width: 96px;
        flex: none;
    }
    .unit-field small {
        position: absolute;
        right: 8px;
        top: 50%;
        transform: translateY(-50%);
        font-size: 13px;
        color: var(--ink-faint);
        pointer-events: none;
    }
    .help {
        grid-column: 1 / -1;
        margin: 0;
        font-size: 11px;
        color: var(--ink-soft);
    }
    .buttons {
        grid-column: 1 / -1;
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }
    .quiet,
    .primary {
        height: 32px;
        padding: 0 14px;
        border-radius: 6px;
        font-size: 13px;
        font-weight: 600;
    }
    .quiet {
        border: 1px solid var(--line-strong);
        background: var(--panel);
        color: var(--ink);
    }
    .primary {
        border: 0;
        background: var(--amber);
        color: var(--on-amber);
    }
</style>
