<!-- The panel of "Where": the search area, and the radius when a sentence gave one. An area that names no place yet, such
     as "Near a place" before its name, stays in the panel until it has one. -->
<script lang="ts">
    import type { Quantity, Where } from '../../lib/planner/search/types';
    import QueryAreaPicker from './QueryAreaPicker.svelte';

    let { where, radius, days, selection, onchange }: {
        where: Where; radius?: Quantity; days: number[]; selection?: Where;
        /** A new area, or a new radius. */
        onchange: (change: { where: Where } | { radius: Quantity | undefined }) => void;
    } = $props();

    /** The area with no name yet, and the area that it was made from: a new area ends it. */
    let draft = $state.raw<{ from: Where; area: Where }>();
    const shown = $derived(draft?.from === where ? draft.area : where);
    const named = (area: Where) => [...area.near ?? [], area.before, area.after].every(point => !point || !('name' in point) || !!point.name.trim());
    function change(area: Where) {
        draft = named(area) ? undefined : { from: where, area };
        if (!draft) onchange({ where: area });
    }
</script>

<div class="where">
    <QueryAreaPicker value={shown} {days} {selection} onChange={change} />
    {#if radius}
        <label class="half">Within, in km<input type="number" min="0.1" max="100" step="0.1" value={radius.value}
            onchange={event => { const value = event.currentTarget.valueAsNumber; onchange({ radius: value > 0 ? { value, unit: 'km' } : undefined }); }} /></label>
    {/if}
</div>

<style>
    .where {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px;
        width: 420px;
        max-width: 100%;
        padding: 12px;
        font-size: 13px;
    }
    .where :global(label) {
        display: flex;
        flex-direction: column;
        grid-column: 1 / -1;
        gap: 6px;
        min-width: 0;
        color: var(--ink-soft);
        font-size: 12px;
        font-weight: 500;
    }
    .where :global(label.half) {
        grid-column: auto;
    }
    .where :global(input), .where :global(select) {
        box-sizing: border-box;
        width: 100%;
        min-width: 0;
        min-height: 36px;
        padding: 6px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 400 13px var(--sans);
    }
</style>
