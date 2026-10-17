<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import { radioKey } from './Segmented.svelte';
    import { difference, optionLetter, type Compared } from '../../lib/planner/route-options';

    let { label, rows, status, retry, note, applyLabel, previewId, onPreview, onApply, onDelete }: {
        /** The name of the list, such as "Route options". */
        label: string;
        /** Your route first, then the routes it is compared with. */
        rows: Compared[];
        /** Shown instead of the rows while they are not known. */
        status: string | null;
        /** Offers Retry after the status or the note. */
        retry?: () => void;
        /** The line under the rows. */
        note: string;
        applyLabel: string;
        /** The previewed row; null shows your route alone. */
        previewId: string | null;
        onPreview: (id: string | null) => void;
        onApply: (row: Compared) => void;
        /** Offers Delete on a previewed row. */
        onDelete?: (row: Compared) => void;
    } = $props();

    const checked = $derived(rows.find(row => row.id === previewId) ?? rows[0]);
    const previewed = $derived(checked && checked !== rows[0] ? checked : undefined);
    let group = $state<HTMLDivElement>();
    let statusBox = $state<HTMLDivElement>();
    const km = (value: number) => value.toFixed(1);

    function notes(row: Compared): string[] {
        return [row.unknownSurfaceKm >= .05 ? `${km(row.unknownSurfaceKm)} km unknown surface` : '', row.climb === null ? 'Elevation incomplete' : ''].filter(Boolean);
    }

    function ariaLabel(row: Compared, index: number): string {
        const d = index ? `. ${difference(row, rows[0])}` : '';
        return `${optionLetter(index)}, ${row.name}: ${km(row.km)} km, climb ${row.climb === null ? 'incomplete' : `${row.climb} m`}, unpaved ${km(row.unpavedKm)} km${d}`;
    }

    /** Ends the preview, or applies or deletes the row, and focuses the checked radio: then the rider's current route. */
    async function end(row?: Compared, apply = onApply) {
        if (row) apply(row);
        else onPreview(null);
        await tick();
        group?.querySelector<HTMLElement>('[role="radio"][tabindex="0"]')?.focus();
    }

    // Retry replaces its button with the loading state, so the state, or else the list, keeps the focus.
    async function again() {
        retry?.();
        await tick();
        (statusBox ?? group)?.focus();
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

{#if status === null}
    <div class="ways">
        <div class="columns" aria-hidden="true"><span>Distance</span><span>Climb</span><span>Unpaved</span></div>
        <div role="radiogroup" aria-label={label} tabindex="-1" bind:this={group} onkeydown={escape}>
            {#each rows as row, index (row.id)}
                {@const quiet = notes(row)}
                <div class="option" class:previewed={row === previewed}>
                    <button type="button" role="radio" aria-checked={row === checked} tabindex={row === checked ? 0 : -1} aria-label={ariaLabel(row, index)}
                        onclick={() => onPreview(index ? row.id : null)} onkeydown={event => key(event, index)}>
                        <span class="letter" class:current={!index}>{optionLetter(index)}</span>
                        <span class="name">
                            <strong>{row.name}</strong>
                            {#if row === previewed}<small>Previewing</small>{:else if row.detail}<small>{row.detail}</small>{/if}
                        </span>
                        <span class="value">{km(row.km)} km</span>
                        <span class="value">{row.climb === null ? '–' : `${row.climb} m`}</span>
                        <span class="value quiet">{km(row.unpavedKm)} km</span>
                        {#if index || quiet.length}
                            <span class="detail">{#if index}<span class="difference">{difference(row, rows[0])}</span>{/if}{#if quiet.length}<small>{quiet.join(' · ')}</small>{/if}</span>
                        {/if}
                    </button>
                    {#if row === previewed}
                        <div class="actions">
                            <button type="button" class="use" onclick={() => end(row)}>{applyLabel}</button>
                            <button type="button" class="planner-action" onclick={() => end()}>Cancel</button>
                            {#if onDelete}<button type="button" class="planner-action quiet delete" onclick={() => end(row, onDelete)}><Icon name="trash" size={14} />Delete</button>{/if}
                        </div>
                    {/if}
                </div>
            {/each}
        </div>
        <p class="note">{note}</p>
        {#if retry}<button type="button" class="planner-action" onclick={again}>Retry</button>{/if}
    </div>
{:else}
    <div class="ways state" role="status" tabindex="-1" bind:this={statusBox}>
        <p>{status}</p>
        {#if retry}<button type="button" class="planner-action" onclick={again}>Retry</button>{/if}
    </div>
{/if}
<span class="sr-only" role="status">{previewed ? `Previewing ${previewed.name}. Your route is unchanged.` : ''}</span>

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
    .delete {
        gap: 6px;
        margin-left: auto;
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
    .ways > .planner-action {
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
