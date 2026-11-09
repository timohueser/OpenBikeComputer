<script lang="ts">
    import { tick, untrack } from 'svelte';
    import Segmented from './Segmented.svelte';
    import { maxRidingDays, type Stay, type Trip } from '../../lib/planner/editor';

    let { trip, dayCount, editing = $bindable(false), onApply }: {
        trip: Trip;
        /** Calendar days, rest days included. */
        dayCount: number;
        editing?: boolean;
        /** `days` is absent when the rider did not edit the day count. */
        onApply: (days: number | undefined, stay: Stay[] | undefined) => void;
    } = $props();
    type Sleep = 'camping' | 'hotel' | 'either';
    const sleepOptions: { value: Sleep; label: string }[] = [{ value: 'camping', label: 'Camping' }, { value: 'hotel', label: 'Hotel' }, { value: 'either', label: 'Either' }];
    const sleepLabels: Record<Sleep, string> = { camping: 'Camping', hotel: 'Hotel', either: 'Camping or hotel' };
    const stays: Record<Sleep, Stay[] | undefined> = { camping: ['camp'], hotel: ['hut', 'hostel', 'hotel'], either: undefined };
    const sleepOf = (stay: Stay[] | undefined): Sleep => stay?.join() === 'camp' ? 'camping' : stay?.join() === 'hut,hostel,hotel' ? 'hotel' : 'either';

    let days = $state(3);
    let daysEdited = $state(false);
    let sleep = $state<Sleep>('either');
    let form = $state<HTMLFormElement>();
    let trigger = $state<HTMLButtonElement>();

    const summary = $derived([
        `${dayCount} ${dayCount === 1 ? 'day' : 'days'}`,
        sleepLabels[sleepOf(trip.stay)],
    ].join(' · '));
    const valid = $derived(Number.isInteger(days) && days >= 1 && days <= maxRidingDays);

    $effect(() => {
        if (!editing) return;
        // The form starts from the trip once per opening; later trip edits must not reset what the rider typed.
        untrack(() => {
            days = trip.days;
            daysEdited = false;
            sleep = sleepOf(trip.stay);
        });
        tick().then(() => form?.querySelector('input')?.focus());
    });

    function apply() {
        if (!valid) return;
        // A stay that the three choices cannot show stays until the rider picks another choice.
        onApply(daysEdited ? days : undefined, sleep === sleepOf(trip.stay) ? trip.stay : stays[sleep]);
        void close();
    }

    async function close() {
        editing = false;
        await tick();
        trigger?.focus();
    }
</script>

{#if trip.mode === 'route'}
    <p class="plan-line">Route</p>
{:else if !editing}
    <p class="plan-line">
        <span>{summary}</span>
        <button type="button" class="planner-action" bind:this={trigger} onclick={() => editing = true} aria-label="Edit day plan">Edit plan</button>
    </p>
{:else}
    <!-- Escape bubbles from the fields; the form closes the editor in place. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form class="plan-editor" bind:this={form} aria-label="Day plan"
        onsubmit={(event) => { event.preventDefault(); apply(); }}
        onkeydown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); void close(); } }}>
        <label class="wide">
            <span>Riding days</span>
            <input type="number" min="1" max={maxRidingDays} required bind:value={days} oninput={() => daysEdited = true} />
        </label>
        <div class="wide">
            <span class="field-label">Where you sleep</span>
            <Segmented label="Where you sleep" options={sleepOptions} value={sleep} onChange={value => sleep = value} />
        </div>
        <div class="buttons">
            <button type="button" class="quiet" onclick={close}>Cancel</button>
            <button type="submit" class="primary" disabled={!valid}>Apply</button>
        </div>
    </form>
{/if}

<style>
    .plan-line {
        display: flex;
        align-items: center;
        gap: 12px;
        margin: 0;
        padding: 0 16px 10px;
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
    .plan-line button { flex: none; white-space: nowrap; }
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
    input {
        width: 96px;
        height: 32px;
        padding: 0 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 600 13px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .field-label { display: block; margin-bottom: 4px; font-size: 13px; color: var(--ink-soft); }
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
