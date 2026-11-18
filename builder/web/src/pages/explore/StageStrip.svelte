<!-- The strip of the parts of a trail: a cell in the colour of the trail for each part in the catalog, a grey dashed cell
     for each part that is not, each as wide as its length. Cells too thin for that are one bar with filled and empty
     runs. -->
<script lang="ts">
    import { onRouteColor } from '../../lib/planner/route-colors';
    import type { TrailPart } from '../../lib/planner/signed-routes';
    import { stripCells } from './strip';

    let { parts, numbered, color }: { parts: TrailPart[]; numbered: boolean; color: string } = $props();

    /** The numbers show only when each cell is wide enough for its number. A cell under `THIN_PX` makes the strip one bar. */
    const DIGIT_PX = 6, PAD_PX = 4, GAP_PX = 1, JUMP_PX = 8, THIN_PX = 4;
    const cells = $derived(stripCells(parts, numbered));
    let width = $state(0);
    /** The width of the cells: the strip less the gaps between them. */
    const room = $derived(width - GAP_PX * (cells.length - 1) - JUMP_PX * cells.slice(1).filter(cell => cell.jump).length);
    const thin = $derived(cells.some(cell => room * cell.share < THIN_PX));
    const numbers = $derived(!thin && cells.every(cell => cell.number === undefined || room * cell.share >= PAD_PX + DIGIT_PX * String(cell.number).length));
</script>

<div class="strip" class:thin style:--trail={color} style:--on-trail={onRouteColor(color)} bind:clientWidth={width} aria-hidden="true">
    {#each cells as cell, i (i)}
        <span class="cell" class:missing={cell.missing} class:jump={cell.jump} style:flex-grow={cell.share}>
            {#if numbers}{cell.number}{/if}
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
    .thin {
        gap: 0;
        overflow: hidden;
        border-radius: 3px;
        background: var(--parchment-2);
    }
    .thin .cell {
        border: 0;
        border-radius: 0;
    }
</style>
