<script lang="ts">
    import { tick } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
    import RouteRow, { hint, stopSign } from './RouteRow.svelte';
    import type { RoutePoint } from '../../lib/planner/editor';
    import { formatAlong } from '../../lib/settings/format';

    let { stops, onInspect, onReorder, measured = true, loop = false, hoveredId = null, onHover }: {
        hoveredId?: string | null;
        /** The route returns to the start, which is its last row too. */
        loop?: boolean;
        onHover?: (id: string | null) => void;
        measured?: boolean;
        stops: { point: RoutePoint; distance: number }[];
        onInspect: (point: RoutePoint) => void;
        onReorder: (id: string, offset: number) => void;
    } = $props();

    const helpId = $props.id();
    const listed = $derived(stops.filter(stop => stop.point.kind !== 'via'));
    // The rows between the first and the last can move.
    const last = $derived(listed.length - 1);
    let list: HTMLOListElement;
    let drag = $state<{ id: string; from: number; to: number; y: number; delta: number; centers: number[] } | null>(null);
    let announcement = $state('');

    async function move(id: string, from: number, to: number, restoreFocus = false) {
        if (from === to || to < 1 || to >= last) return;
        const label = listed[from].point.label;
        onReorder(id, to - from);
        announcement = `${label} moved to stop ${to} of ${last - 1}. Changed legs follow roads.`;
        if (restoreFocus) {
            await tick();
            [...list.querySelectorAll<HTMLButtonElement>('.handle')].find(handle => handle.dataset.point === id)?.focus();
        }
    }

    function startDrag(event: PointerEvent, id: string, index: number) {
        if (event.button !== 0 || last < 3) return;
        const centers = [...list.children].map(row => { const rect = row.getBoundingClientRect(); return rect.top + rect.height / 2; });
        drag = { id, from: index, to: index, y: event.clientY, delta: 0, centers };
        event.currentTarget instanceof HTMLElement && event.currentTarget.setPointerCapture(event.pointerId);
    }

    function dragMove(event: PointerEvent) {
        if (!drag) return;
        drag.delta = event.clientY - drag.y;
        drag.to = drag.centers.reduce((nearest, center, index) =>
            index > 0 && index < last && Math.abs(center - event.clientY) < Math.abs(drag!.centers[nearest] - event.clientY) ? index : nearest, drag.from);
    }

    function drop(event: PointerEvent) {
        if (!drag) return;
        const bounds = list.getBoundingClientRect();
        if (Math.abs(drag.delta) > 4 && event.clientX >= bounds.left && event.clientX <= bounds.right && event.clientY >= bounds.top && event.clientY <= bounds.bottom) {
            move(drag.id, drag.from, drag.to);
        }
        drag = null;
    }
</script>

<svelte:window onkeydown={(event) => { if (event.key === 'Escape') drag = null; }} onblur={() => drag = null} />

<ol class="route" bind:this={list}>
    {#each listed as { point, distance }, index (index === last && loop ? 'end' : point.id)}
        <RouteRow sign={index === 0 ? 'start' : index === last ? 'finish' : stopSign(point)} label={point.label}
            along={index === 0 ? 'Start' : measured ? formatAlong(distance * 1000) : index === last ? 'Finish' : ''}
            lit={hoveredId === point.id} onclick={() => onInspect(point)} onHover={on => onHover?.(on ? point.id : null)}
            class={{ dragging: drag?.id === point.id && index < last && Math.abs(drag.delta) > 4, 'drop-before': drag?.to === index && drag.to < drag.from, 'drop-after': drag?.to === index && drag.to > drag.from }}
            style={`--drag-y: ${drag?.id === point.id && index < last ? drag.delta : 0}px`}>
            {#if index > 0 && index < last}
                <button type="button" class="handle" data-point={point.id} aria-label={`Reorder ${point.label}`} aria-describedby={helpId}
                    title="Drag to reorder · use ↑ or ↓ when focused" disabled={last < 3}
                    onpointerdown={(event) => startDrag(event, point.id, index)} onpointermove={dragMove}
                    onpointerup={drop} onpointercancel={() => drag = null}
                    onkeydown={(event) => {
                        if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
                            event.preventDefault();
                            move(point.id, index, index + (event.key === 'ArrowUp' ? -1 : 1), true);
                        }
                    }}><Icon name="grip" size={18} /></button>
            {/if}
        </RouteRow>
        {#if index === 0 && last === 1}<RouteRow {...hint} />{/if}
    {/each}
</ol>
<span class="sr-only" id={helpId}>Drag to reorder. Use the up and down arrow keys when focused.</span>
<span class="sr-only" role="status">{announcement}</span>

<style>
    .route {
        margin: 0;
        padding: 4px 12px 16px;
        list-style: none;
    }
    /* Each row keeps the column of the handle, so the distances line up. */
    .route > :global(li) { grid-template-columns: minmax(0, 1fr) 24px; }
    .route > :global(li.dragging) { z-index: 1; transform: translateY(var(--drag-y)); background: var(--panel); box-shadow: var(--planner-shadow); pointer-events: none; }
    .route > :global(li.dragging::before) { display: none; }
    .route > :global(li.drop-before::after), .route > :global(li.drop-after::after) { content: ''; position: absolute; left: 0; right: 0; height: 2px; background: var(--forest); }
    .route > :global(li.drop-before::after) { top: -1px; }
    .route > :global(li.drop-after::after) { bottom: -1px; }
    .handle {
        display: grid;
        place-items: center;
        width: 24px;
        height: 36px;
        padding: 0;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-faint);
        font: inherit;
        cursor: grab;
        touch-action: none;
    }
    .handle:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
    .handle:active { cursor: grabbing; }
    .handle:hover:not(:disabled) { background: var(--parchment-2); color: var(--ink-soft); }
    .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
</style>
