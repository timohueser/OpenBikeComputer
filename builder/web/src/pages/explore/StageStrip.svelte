<!-- The strip of the parts of a trail: a cell in the colour of the trail for each part in the catalog, a grey dashed cell
     for each part that is not, each as wide as its length. Narrow cells make one bar with filled and empty runs. With
     `marked`, that part keeps the full colour and the others are paler. -->
<script lang="ts">
    import { onRouteColor } from '../../lib/planner/route-colors';
    import type { TrailPart } from '../../lib/planner/signed-routes';
    import { stripCells } from './strip';

    let { parts, numbered, color, marked }: { parts: TrailPart[]; numbered: boolean; color: string; marked?: number } = $props();

    /** The numbers show only when each cell is wide enough for its number. With no numbers, cells under `CELL_PX` on
     * average make the strip one bar with filled and empty runs. */
    const DIGIT_PX = 6, PAD_PX = 4, GAP_PX = 1, JUMP_PX = 8, CELL_PX = 12;
    const cells = $derived(stripCells(parts, numbered));
    let width = $state(0);
    /** The width of the cells: the strip less the gaps between them. */
    const room = $derived(width - GAP_PX * (cells.length - 1) - JUMP_PX * cells.slice(1).filter(cell => cell.jump).length);
    const numbers = $derived(numbered && cells.every(cell => cell.number === undefined || room * cell.share >= PAD_PX + DIGIT_PX * String(cell.number).length));
    const thin = $derived(!numbers && room / cells.length < CELL_PX);
</script>

<div class="strip" class:thin style:--trail={color} style:--on-trail={onRouteColor(color)} bind:clientWidth={width} aria-hidden="true">
    {#each cells as cell, i (i)}
        <span class="cell" class:missing={cell.missing} class:jump={cell.jump} class:pale={marked !== undefined && cell.id !== marked} style:flex-grow={cell.share}>
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
    .pale {
        opacity: 0.4;
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
