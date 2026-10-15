<script lang="ts">
    import { tick } from 'svelte';
    import { radioKey } from './Segmented.svelte';
    import type { RouteOptions } from '../../lib/planner/plan-session.svelte';
    import { climbM, difference, elevationComplete, optionLetter, optionName, unpavedKm } from '../../lib/planner/route-options';
    import { presetName } from '../../lib/planner/riding-profiles';
    import type { EngineRoute } from '../../lib/planner/routing';

    let { options, previewId, places, onPreview, onApply, onRetry }: {
        options: RouteOptions;
        /** The previewed option; null shows the current route alone. */
        previewId: string | null;
        /** Settlement names of corridor options, by route id. */
        places: Record<string, string>;
        onPreview: (id: string | null) => void;
        onApply: (route: EngineRoute, name: string) => void;
        onRetry: () => void;
    } = $props();

    const rows = $derived(options.state === 'ready' ? options.routes : []);
    const checked = $derived(rows.find(route => route.id === previewId) ?? rows[0]);
    const previewed = $derived(checked && checked !== rows[0] ? checked : undefined);
    const messages = {
        unavailable: 'Route options need one routed line from start to finish.',
        loading: 'Finding route options…',
        stale: 'Your plan changed. Finding route options for it…',
    };
    let group = $state<HTMLDivElement>();
    let status = $state<HTMLDivElement>();
    const km = (metres: number) => (metres / 1000).toFixed(1);

    function label(route: EngineRoute, index: number): string {
        const d = index ? difference(route, rows[0]) : null, climb = elevationComplete(route) ? `${climbM(route)} m` : 'incomplete';
        return `${optionLetter(index)}, ${index ? optionName(route, places) : 'Your route'}: ${km(route.totals.distance_m)} km, climb ${climb}, unpaved ${unpavedKm(route).toFixed(1)} km${d ? `. ${d}` : ''}`;
    }

    /** Ends the preview, or applies the option, and focuses the checked radio: then the rider's current route. */
    async function end(route?: EngineRoute) {
        if (route) onApply(route, optionName(route, places));
        else onPreview(null);
        await tick();
        group?.querySelector<HTMLElement>('[role="radio"][tabindex="0"]')?.focus();
    }

    // Retry replaces its button with the loading state, so the state keeps the focus.
    async function retry() {
        onRetry();
        await tick();
        status?.focus();
    }

    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || !previewed) return;
        // The page's Escape would also clear the map selection.
        event.stopPropagation();
        void end();
    }

    function key(event: KeyboardEvent, index: number) {
        if (event.key === 'Enter' && index) { event.preventDefault(); void end(rows[index]); }
        else radioKey(event, index, rows.length, next => onPreview(next ? rows[next].id : null));
    }
</script>

{#if options.state === 'ready'}
    <div class="ways">
        <div class="columns" aria-hidden="true"><span>Distance</span><span>Climb</span><span>Unpaved</span></div>
        <div role="radiogroup" aria-label="Route options" tabindex="-1" bind:this={group} onkeydown={escape}>
            {#each rows as route, index (route.id)}
                {@const d = index ? difference(route, rows[0]) : null}
                {@const notes = [route.totals.surface_m[0] ? `${km(route.totals.surface_m[0])} km unknown surface` : '', elevationComplete(route) ? '' : 'Elevation incomplete'].filter(Boolean)}
                <div class="option" class:previewed={route === previewed}>
                    <button type="button" role="radio" aria-checked={route === checked} tabindex={route === checked ? 0 : -1} aria-label={label(route, index)}
                        onclick={() => onPreview(index ? route.id : null)} onkeydown={event => key(event, index)}>
                        <span class="letter" class:current={!index}>{optionLetter(index)}</span>
                        <span class="name">
                            <strong>{index ? optionName(route, places) : 'Your route'}</strong>
                            {#if !index}<small>{presetName(route.profile)}</small>{:else if route === previewed}<small>Previewing</small>{/if}
                        </span>
                        <span class="value">{km(route.totals.distance_m)} km</span>
                        <span class="value">{elevationComplete(route) ? `${climbM(route)} m` : '–'}</span>
                        <span class="value quiet">{unpavedKm(route).toFixed(1)} km</span>
                        {#if d || notes.length}
                            <span class="detail">{#if d}<span class="difference">{d}</span>{/if}{#if notes.length}<small>{notes.join(' · ')}</small>{/if}</span>
                        {/if}
                    </button>
                    {#if route === previewed}
                        <div class="actions">
                            <button type="button" class="use" onclick={() => end(route)}>Use this route</button>
                            <button type="button" class="planner-action" onclick={() => end()}>Cancel</button>
                        </div>
                    {/if}
                </div>
            {/each}
        </div>
        <p class="note">{rows.length < 2 ? 'No useful alternative to your route was found.' : 'Differences compare with your route.'}</p>
    </div>
{:else}
    <div class="ways state" role="status" tabindex="-1" bind:this={status}>
        <p>{options.state === 'error' ? options.message : messages[options.state]}</p>
        {#if options.state === 'error'}<button type="button" class="planner-action" onclick={retry}>Retry</button>{/if}
    </div>
{/if}
<span class="sr-only" role="status">{previewed ? `Previewing ${optionName(previewed, places)}. Your route is unchanged.` : ''}</span>

<style>
    .ways {
        --grid: 24px minmax(0, 1fr) 64px 52px 56px;
        padding: 4px 16px 16px;
    }
    .columns,
    [role="radio"] {
        display: grid;
        grid-template-columns: var(--grid);
        column-gap: 8px;
        align-items: baseline;
    }
    .columns {
        padding: 0 8px 4px;
        font-size: 12px;
        color: var(--ink-soft);
        text-align: right;
    }
    .columns span:first-child {
        grid-column: 3;
    }
    .option {
        margin: 0 -8px;
        padding: 0 8px;
        border-radius: 6px;
        border-bottom: 1px solid var(--line);
    }
    .option.previewed {
        padding-bottom: 12px;
        background: var(--parchment-2);
        border-bottom-color: transparent;
    }
    [role="radio"] {
        width: calc(100% + 16px);
        margin: 0 -8px;
        padding: 10px 8px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink);
        text-align: left;
        font: inherit;
        cursor: pointer;
    }
    .option:not(.previewed) [role="radio"]:hover {
        background: var(--parchment-2);
    }
    .letter {
        grid-row: span 2;
        display: inline-grid;
        place-items: center;
        width: 20px;
        height: 20px;
        border: 2px solid var(--ink);
        border-radius: 50%;
        font: 700 11px var(--sans);
    }
    .letter.current {
        border-color: var(--route);
        background: var(--route);
        color: var(--panel);
    }
    .previewed .letter {
        background: var(--ink);
        color: var(--panel);
    }
    .name {
        min-width: 0;
    }
    strong,
    small {
        display: block;
    }
    strong {
        font: 600 14px var(--sans);
    }
    small {
        margin-top: 2px;
        font: 400 12px var(--sans);
        color: var(--ink-soft);
    }
    .value {
        text-align: right;
        font: 700 13px var(--mono);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .value.quiet {
        font-weight: 400;
    }
    .detail {
        grid-column: 2 / -1;
        margin-top: 4px;
    }
    .difference {
        display: block;
        font: 400 13px var(--sans);
    }
    .actions {
        display: flex;
        gap: 8px;
        padding-left: 32px;
    }
    .use {
        min-height: 32px;
        padding: 0 14px;
        border: 0;
        border-radius: 6px;
        background: var(--amber);
        color: var(--on-amber);
        font: 600 13px var(--sans);
    }
    .use:hover {
        filter: brightness(.95);
    }
    .note,
    .state p {
        margin: 8px 0 0;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .state:focus {
        outline: none;
    }
    .state .planner-action {
        margin-top: 8px;
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
