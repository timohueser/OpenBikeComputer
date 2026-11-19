<!-- The number of riding days, and the only control that sets the type: 1 day is a route, 2 or more a multi-day trip. -->
<script lang="ts">
    import { tick } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import { fewerDays, maxRidingDays, multiDay, routeToTrip, setDays, type Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { trip, line, onChange }: { trip: Trip; line: RoutingLine | undefined; onChange: (next: Trip, description: string) => void } = $props();

    const days = $derived(multiDay(trip) ? trip.days : 1);
    const text = (days: number) => `${days} riding ${days === 1 ? 'day' : 'days'}`;
    let fewerButton: HTMLButtonElement, moreButton: HTMLButtonElement;

    // A focused button that becomes disabled drops the focus to the body.
    async function keepFocus(pressed: HTMLButtonElement, other: HTMLButtonElement) {
        const focused = document.activeElement === pressed;
        await tick();
        if (focused && pressed.disabled) other.focus();
    }

    function more() {
        void keepFocus(moreButton, fewerButton);
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

<div class="days" role="group" aria-label="Number of riding days">
    <button type="button" aria-label="One day fewer" bind:this={fewerButton} disabled={days === 1} onclick={fewer}><Icon path="M5 12h14" size={15} /></button>
    <span class="count"><span class:guess={days > 1 && !trip.daysSet}>{text(days)}</span></span>
    <button type="button" aria-label="One day more" bind:this={moreButton} disabled={days >= maxRidingDays} onclick={more}><Icon name="plus" size={15} /></button>
</div>

<style>
    .days {
        display: flex;
        align-items: center;
        gap: 2px;
        padding: 2px;
        border: 1px solid var(--line-strong);
        border-radius: 8px;
    }
    button {
        display: grid;
        place-items: center;
        width: 30px;
        height: 30px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        cursor: pointer;
    }
    button:disabled { cursor: default; opacity: .45; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    button:not(:disabled):hover { background: var(--parchment-2); }
    .count {
        min-width: 92px;
        text-align: center;
        font-size: 14px;
        color: var(--ink);
        font-variant-numeric: tabular-nums;
    }
    .count span {
        padding-bottom: 1px;
        border-bottom: 2px solid var(--ink);
        font-weight: 700;
    }
    .count span.guess {
        border-bottom: 1.5px dashed var(--ink-soft);
        font-weight: 600;
    }
</style>
