<script lang="ts">
    import type { Snippet } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { category, kindLabel } from '../../lib/planner/search/presentation';
    import { proposalTitle, stopCounts, suits } from '../../lib/planner/stops';
    import type { Day } from '../../lib/planner/editor';
    import type { NightChoices } from '../../lib/planner/night-stops.svelte';

    let { day, label, nights, children }: {
        /** The riding day that ends at this night. */
        day: Day;
        /** Calendar number of the night. */
        label: number;
        nights: NightChoices;
        /** The towns, shown while this night is chosen. */
        children: Snippet;
    } = $props();

    const pinned = $derived(day.pinned);
    const open = $derived(nights.choosing === day.number);
    const around = $derived(pinned ? undefined : nights.around(day.number));
    const [earlier, later] = $derived(pinned ? [undefined, undefined] : nights.neighbours(day.number));
    const found = $derived(nights.found(day.number));
    const icon = $derived(pinned?.placeKind ? ({ camp: 'camp', hotel: 'hotel' } as Record<string, string>)[category(pinned.placeKind)] ?? 'moon' : 'moon');
    const towns = $derived(found?.state === 'ready' ? found.stops.filter(stop => suits(stop, nights.sleep)).length : 0);
    const wanted = $derived(nights.sleep === 'camping' ? 'campsite' : nights.sleep === 'hotel' ? 'hotel' : 'town with a place to sleep');
    const status = $derived(found?.state === 'loading' ? 'Finding towns nearby…' : found?.state === 'error' ? 'Towns could not load'
        : towns ? `No ${wanted} near the day end · ${towns} ${towns === 1 ? 'town' : 'towns'} to choose from` : `No ${wanted} nearby`);
    const detail = $derived(!around ? status
        : nights.sleep === 'camping' && around.campsite ? `${around.stop.name} · ${around.campsite.off < .15 ? 'on the route' : `${around.campsite.off.toFixed(1)} km off the route`}`
        : around.campsite ? `${around.campsite.place.label} · ${stopCounts(around.stop)}` : stopCounts(around.stop));
</script>

<div class="night" class:chosen={!!pinned} class:open {@attach node => { if (open) node.scrollIntoView({ block: 'start', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' }); }}>
    <span class="mark" aria-hidden="true">{#if pinned}<Icon name={icon} size={14} />{:else}{label}{/if}</span>
    <div class="text">
        {#if pinned}
            <strong>{pinned.label}</strong>
            <small>Night {label}{pinned.placeKind ? ` · ${kindLabel(pinned.placeKind)}` : ''}</small>
        {:else}
            <strong>{around ? proposalTitle(around, nights.sleep) : `Night ${label}`}</strong>
            <small><span class="sr-only">Suggested. </span>{detail}</small>
        {/if}
    </div>
    {#if !pinned && !open}
        <span class="step">
            <button type="button" class="icon" disabled={!earlier} aria-label={earlier ? `Move night ${label} to ${earlier.name}` : 'No earlier town'} onclick={() => nights.step(day.number, -1)}><Icon name="back" size={15} /></button>
            <button type="button" class="icon" disabled={!later} aria-label={later ? `Move night ${label} to ${later.name}` : 'No later town'} onclick={() => nights.step(day.number, 1)}><Icon name="arrow" size={15} /></button>
        </span>
    {/if}
    <button type="button" class="choose" aria-expanded={open} aria-label={`${open ? 'Close' : pinned ? 'Change' : 'Choose'} night ${label}`}
        onclick={() => nights.choose(open ? null : day.number)}>{open ? 'Close' : pinned ? 'Change' : 'Choose'}</button>
</div>
{#if open}
    <div class="towns">{@render children()}</div>
{/if}

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
    .towns {
        margin: 0 0 8px 36px;
        padding: 4px 0 8px;
    }
</style>
