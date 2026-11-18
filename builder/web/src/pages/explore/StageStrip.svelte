<!-- The strip of the parts of a trail: a cell in the colour of the trail for each part in the catalog, a grey dashed cell
     for each part that is not, each as wide as its length. -->
<script lang="ts">
    import { onRouteColor } from '../../lib/planner/route-colors';
    import type { TrailPart } from '../../lib/planner/signed-routes';
    import { stripCells } from './strip';

    let { parts, numbered, color }: { parts: TrailPart[]; numbered: boolean; color: string } = $props();

    /** A number shows only in a cell that is wide enough for it. */
    const DIGIT_PX = 6, PAD_PX = 4;
    const cells = $derived(stripCells(parts, numbered));
    let width = $state(0);
</script>

<div class="strip" style:--trail={color} style:--on-trail={onRouteColor(color)} bind:clientWidth={width} aria-hidden="true">
    {#each cells as cell, i (i)}
        <span class="cell" class:missing={cell.missing} class:jump={cell.jump} style:flex-grow={cell.share}>
            {#if cell.number !== undefined && width * cell.share >= PAD_PX + DIGIT_PX * String(cell.number).length}{cell.number}{/if}
        </span>
    {/each}
</div>

<style>
    .strip {
        display: flex;
        gap: 1px;
        height: 22px;
    }
    .cell {
        display: grid;
        flex-basis: 0;
        place-items: center;
        min-width: 0;
        overflow: hidden;
        border-radius: 3px;
        background: var(--trail);
        color: var(--on-trail);
        font: 600 11px/1 var(--sans);
    }
    .missing {
        border: 1px dashed var(--line-strong);
        background: none;
        color: var(--ink-soft);
    }
    .jump:not(:first-child) {
        margin-left: 8px;
    }
</style>
