<!-- PROTOTYPE (prototype/trip-bar): the choice between a one-day route and a multi-day trip at the head of the stops.
     T1 is a two-part switch; T2 is the day count, where one day is the one-day route. -->
<script lang="ts">
    import Icon from '../../lib/ui/Icon.svelte';
    import Segmented from '../../lib/ui/Segmented.svelte';
    import { fewestDays, maxRidingDays, setDays, type Trip } from '../../lib/planner/editor';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { variant, trip, line, onChange, onPlan, look = '', onOptions }: {
        variant: string;
        /** PROTOTYPE: S1 makes it the head of the stops; S2 and S3 show only the stepper. */
        look?: string;
        /** S1: "Route options" on the right of the head. */
        onOptions?: () => void;
        trip: Trip;
        line: RoutingLine | undefined;
        onChange: (change: Partial<Trip>, description: string) => void;
        onPlan: (next: Trip, description: string) => void;
    } = $props();

    const multi = $derived(trip.mode !== 'route');
    const days = $derived(multi ? trip.days : 1);

    function more() {
        if (!multi) onPlan(setDays({ ...trip, mode: 'trip' }, 2), 'Multi-day trip, 2 riding days');
        else onPlan(setDays(trip, trip.days + 1), `${trip.days + 1} riding days`);
    }

    function fewer() {
        if (trip.days <= 2) onChange({ mode: 'route' }, 'One-day route');
        else onPlan(setDays(trip, trip.days - 1), `${trip.days - 1} riding days`);
    }
</script>

{#if variant === 'T1'}
    <div class="type t1">
        <Segmented label="Route type" compact value={multi ? 'trip' : 'route'}
            options={[{ value: 'route', label: 'One day' }, { value: 'trip', label: 'Multi-day' }]}
            onChange={(mode) => onChange({ mode: mode as Trip['mode'] }, mode === 'route' ? 'One-day route' : 'Multi-day trip')} />
    </div>
{:else if look === 'S2' || look === 'S3'}
    {@render stepper()}
{:else if look === 'S1'}
    <div class="type head">
        {@render stepper()}
        {#if onOptions && line}<button type="button" class="options" onclick={onOptions}><Icon name="route" size={15} />Route options</button>{/if}
    </div>
{:else}
    <div class="type t2">{@render stepper()}</div>
{/if}

{#snippet stepper()}
    <div class="days" role="group" aria-label="Riding days">
        <button type="button" aria-label="One day fewer" disabled={!multi || (trip.days > 2 && trip.days <= fewestDays(trip))} onclick={fewer}><Icon path="M5 12h14" size={15} /></button>
        <span class="count" class:guess={multi && !trip.daysSet}>{days} {days === 1 ? 'day' : 'days'}</span>
        <button type="button" aria-label="One day more" disabled={days >= maxRidingDays} onclick={more}><Icon name="plus" size={15} /></button>
    </div>
{/snippet}

<style>
    .type { display: flex; align-items: center; flex-wrap: wrap; gap: 8px 12px; padding: 10px 16px; border-top: 1px solid var(--line); font-size: 13px; color: var(--ink-soft); }
    .days { display: inline-flex; align-items: center; gap: 2px; border: 1px solid var(--line-strong); border-radius: 6px; }
    .days button { display: grid; place-items: center; width: 30px; height: 30px; border: 0; background: none; color: var(--ink); cursor: pointer; }
    .days button:disabled { opacity: .35; cursor: default; }
    .days button:not(:disabled):hover { background: var(--parchment-2); }
    .head { padding: 8px 16px; border-top: 1px solid var(--line); }
    .options { display: inline-flex; align-items: center; gap: 6px; margin-left: auto; min-height: 28px; padding: 0 6px; border: 0; border-radius: 6px; background: none; color: var(--ink-soft); font: inherit; cursor: pointer; }
    .options:hover { background: var(--parchment-2); color: var(--ink); }
    .count { min-width: 52px; text-align: center; font-weight: 600; color: var(--ink); }
    .count.guess { font-style: italic; }
</style>
