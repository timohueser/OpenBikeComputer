<!-- The number of days, and the only control that sets the type: 1 day is a route, 2 or more a multi-day trip. -->
<script lang="ts">
    import { tick } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { fewerDays, maxRidingDays, multiDay, routeToTrip, setDays, type Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, onChange, onLimit }: {
        trip: Trip; line: RoutingLine | undefined;
        onChange: (next: Trip, description: string) => void;
        /** A press of + at the most days. */
        onLimit: () => void;
    } = $props();

    const days = $derived(multiDay(trip) ? trip.days : 1);
    const text = (days: number) => `${days} ${days === 1 ? 'day' : 'days'}`;
    let fewerButton: HTMLButtonElement, moreButton: HTMLButtonElement;

    // A focused button that becomes disabled drops the focus to the body.
    async function keepFocus(pressed: HTMLButtonElement, other: HTMLButtonElement) {
        const focused = document.activeElement === pressed;
        await tick();
        if (focused && pressed.disabled) other.focus();
    }

    function more() {
        if (days >= maxRidingDays) return onLimit();
        if (days > 1) return onChange(setDays(trip, days + 1), text(days + 1));
        const next = routeToTrip(trip, line);
        onChange(next, `Multi-day trip, ${text(next.days)}`);
    }

    function fewer() {
        void keepFocus(fewerButton, moreButton);
        const next = fewerDays(trip);
        onChange(next, multiDay(next) ? text(next.days) : 'One-day route · + brings back the days');
    }
</script>

<div class="days" role="group" aria-label="Number of days">
    <button type="button" aria-label="One day fewer" bind:this={fewerButton} disabled={days === 1} onclick={fewer}><Icon path="M5 12h14" size={15} /></button>
    <span class="count"><span class:guess={days > 1 && !trip.daysSet}>{text(days)}</span></span>
    <!-- At the most days the + stays in the focus order, so a press can say why it adds no day. -->
    <button type="button" aria-label="One day more" bind:this={moreButton} aria-disabled={days >= maxRidingDays} onclick={more}><Icon name="plus" size={15} /></button>
</div>

<style>
    /* A well of the trip row (`TripRow`), which takes the free width of the line. */
    .days {
        flex: 1;
        display: flex;
        align-items: center;
        justify-content: space-between;
        min-width: 104px;
        height: 36px;
        padding: 0 2px;
        border-radius: 8px;
        background: var(--parchment-2);
        box-shadow: inset 0 0 0 1px var(--rim, transparent);
    }
    button {
        display: grid;
        place-items: center;
        width: 30px;
        height: 32px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        cursor: pointer;
    }
    button:disabled, button[aria-disabled='true'] { cursor: default; opacity: .4; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 0; }
    button:not(:disabled, [aria-disabled='true']):hover { color: var(--ink); }
    .count {
        white-space: nowrap;
        font: 700 14px var(--sans);
        color: var(--ink);
        font-variant-numeric: tabular-nums;
    }
    /* A guessed count: the planner set it, the rider did not. */
    .count .guess {
        color: var(--ink-soft);
        text-decoration: underline dotted 1.5px;
        text-underline-offset: 4px;
    }
</style>
