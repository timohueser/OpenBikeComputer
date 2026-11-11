<script lang="ts">
    import Icon from './PlannerIcon.svelte';
    import { kindLabel, sleepIcon } from '../../lib/planner/search/presentation';
    import { proposalTitle, stayNouns, stopCounts, suits } from '../../lib/planner/stops';
    import { clockTime, nightLow, sunsetAt } from '../../lib/planner/evening';
    import type { Day } from '../../lib/planner/editor';
    import type { Coordinate } from '../../lib/planner/geo';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';

    let { day, label, nights, end, elevation, onShow, onActive }: {
        /** The riding day that ends at this night. */
        day: Day;
        /** Calendar number of the night. */
        label: number;
        nights: NightChoices;
        /** Where the day ends on the route, and its height there. */
        end?: Coordinate;
        elevation: number | null;
        /** Shows the place of a pinned night. */
        onShow: () => void;
        /** The pointer or the keyboard focus enters or leaves the row. */
        onActive?: (active: boolean) => void;
    } = $props();

    const pinned = $derived(day.pinned);
    const around = $derived(pinned ? undefined : nights.around(day.number));
    const [earlier, later] = $derived(pinned ? [undefined, undefined] : nights.neighbours(day.number));
    const found = $derived(nights.found(day.number));
    const icon = $derived(sleepIcon(pinned?.placeKind));
    const kinds = $derived(nights.kinds(day.number));
    const towns = $derived(found?.state === 'ready' ? found.stops.filter(stop => suits(stop, kinds)).length : 0);
    const wanted = $derived(kinds.length === 1 ? stayNouns[kinds[0]] : 'town with a place to sleep');
    const status = $derived(found?.state === 'loading' ? 'Finding towns nearby…' : found?.state === 'error' ? 'Towns could not load'
        : towns ? `No ${wanted} near the day end · ${towns} ${towns === 1 ? 'town' : 'towns'} to choose from` : `No ${wanted} nearby`);
    const detail = $derived(!around ? status
        : around.place ? `${around.stop.name} · ${around.place.off < .15 ? 'on the route' : `${around.place.off.toFixed(1)} km off the route`}`
        : stopCounts(around.stop, kinds, true));
    const date = $derived(nights.date(day.number));
    const sunset = $derived(date && end ? sunsetAt(end, date) : null);
    let low = $state('');
    $effect(() => {
        low = '';
        if (!date || !end) return;
        const abort = new AbortController();
        nightLow(end, elevation, date, abort.signal).then(text => { if (!abort.signal.aborted) low = text; }, () => {});
        return () => abort.abort();
    });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="night" class:chosen={!!pinned} onmouseenter={() => onActive?.(true)} onmouseleave={() => onActive?.(false)}
    onfocusin={() => onActive?.(true)} onfocusout={() => onActive?.(false)}>
    <span class="mark" aria-hidden="true">{#if pinned}<Icon name={icon} size={14} />{:else}{label}{/if}</span>
    {#if pinned}
        <button type="button" class="text" onclick={onShow}>
            <strong>{pinned.label}</strong>
            <small>Night {label}{pinned.placeKind ? ` · ${kindLabel(pinned.placeKind)}` : ''}</small>
            {@render evening()}
        </button>
    {:else}
        <div class="text">
            <strong>{around ? proposalTitle(around) : `Night ${label}`}</strong>
            <small><span class="sr-only">Suggested. </span>{detail}</small>
            {@render evening()}
        </div>
        {@const back = earlier ? `Move night ${label} to ${earlier.name}` : 'No earlier town'}
        {@const ahead = later ? `Move night ${label} to ${later.name}` : 'No later town'}
        <span class="step">
            <button type="button" class="icon" disabled={!earlier} aria-label={back} title={back} onclick={() => nights.step(day.number, -1)}><Icon name="back" size={15} /></button>
            <button type="button" class="icon" disabled={!later} aria-label={ahead} title={ahead} onclick={() => nights.step(day.number, 1)}><Icon name="arrow" size={15} /></button>
        </span>
    {/if}
    <button type="button" class="choose" data-night={day.number} aria-label={`${pinned ? 'Change' : 'Choose'} night ${label}`} onclick={() => nights.choose(day.number)}>{pinned ? 'Change' : 'Choose'}<Icon name="down" size={13} /></button>
</div>
{#snippet evening()}
    {#if date && end}<small class="evening">{sunset ? `Sunset ${clockTime(sunset)}` : 'No sunset'}{low ? ` · ${low}` : ''}</small>{/if}
{/snippet}

<style>
    .night {
        display: flex;
        align-items: center;
        gap: 12px;
        min-height: 52px;
        padding: 6px 0;
    }
    .mark {
        position: relative;
        z-index: 1;
        width: 24px;
        height: 24px;
        flex: none;
        display: grid;
        place-items: center;
        box-sizing: border-box;
        border: 1.5px dashed var(--ink);
        border-radius: 50%;
        background: var(--panel);
        color: var(--ink);
        font: 700 12px var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .chosen .mark {
        border: 0;
        background: var(--ink);
        color: var(--panel);
    }
    .text {
        flex: 1;
        min-width: 0;
    }
    button.text {
        margin: -4px 0 -4px -6px;
        padding: 4px 6px;
        border-radius: 6px;
        text-align: left;
    }
    button.text:hover {
        background: var(--parchment-2);
    }
    strong,
    small {
        display: block;
        overflow-wrap: anywhere;
    }
    strong {
        font: 600 14px/1.3 var(--sans);
    }
    .night:not(.chosen) strong {
        font-weight: 500;
    }
    small {
        margin-top: 2px;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .evening {
        margin-top: 1px;
        font-size: 12px;
    }
    button {
        border: 0;
        background: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }
    button:focus-visible {
        outline: 2px solid var(--ink);
        outline-offset: 2px;
    }
    .step {
        display: flex;
        flex: none;
    }
    .icon {
        flex: none;
        display: grid;
        place-items: center;
        width: 32px;
        height: 32px;
        border-radius: 6px;
        color: var(--ink-soft);
    }
    .icon:hover:not(:disabled) {
        background: var(--parchment-2);
        color: var(--ink);
    }
    .icon:disabled {
        opacity: .35;
        cursor: default;
    }
    .choose {
        flex: none;
        display: inline-flex;
        align-items: center;
        gap: 6px;
        min-height: 32px;
        padding: 0 12px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        font-size: 13px;
        font-weight: 600;
    }
    .choose:hover {
        background: var(--parchment-2);
    }
    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
        white-space: nowrap;
    }
</style>
