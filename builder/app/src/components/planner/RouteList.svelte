<script lang="ts">
    import { tick } from 'svelte';
    import Icon from './PlannerIcon.svelte';
    import type { RoutePoint } from '../../lib/planner/editor';

    let { stops, onInspect, onReorder, measured = true, hoveredId = null, onHover }: {
        hoveredId?: string | null;
        onHover?: (id: string | null) => void;
        measured?: boolean;
        stops: { point: RoutePoint; distance: number }[];
        onInspect: (point: RoutePoint) => void;
        onReorder: (id: string, offset: number) => void;
    } = $props();

    const helpId = $props.id();
    const listed = $derived(stops.filter(stop => stop.point.kind !== 'via'));
    let list: HTMLOListElement;
    let drag = $state<{ id: string; from: number; to: number; y: number; delta: number; centers: number[] } | null>(null);
    let announcement = $state('');

    async function move(id: string, from: number, to: number, restoreFocus = false) {
        if (from === to || to < 1 || to > listed.length - 2) return;
        const label = listed[from].point.label;
        onReorder(id, to - from);
        announcement = `${label} moved to stop ${to} of ${listed.length - 2}. Changed legs follow roads.`;
        if (restoreFocus) {
            await tick();
            [...list.querySelectorAll<HTMLButtonElement>('.handle')].find(handle => handle.dataset.point === id)?.focus();
        }
    }

    function startDrag(event: PointerEvent, id: string, index: number) {
        if (event.button !== 0 || listed.length < 4) return;
        const centers = [...list.children].map(row => { const rect = row.getBoundingClientRect(); return rect.top + rect.height / 2; });
        drag = { id, from: index, to: index, y: event.clientY, delta: 0, centers };
        event.currentTarget instanceof HTMLElement && event.currentTarget.setPointerCapture(event.pointerId);
    }

    function dragMove(event: PointerEvent) {
        if (!drag) return;
        drag.delta = event.clientY - drag.y;
        drag.to = drag.centers.reduce((nearest, center, index) =>
            index > 0 && index < listed.length - 1 && Math.abs(center - event.clientY) < Math.abs(drag!.centers[nearest] - event.clientY) ? index : nearest, drag.from);
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
    {#each listed as { point, distance }, index (point.id)}
        <li class:dragging={drag?.id === point.id && Math.abs(drag.delta) > 4}
            class:drop-before={drag?.to === index && drag.to < drag.from}
            class:drop-after={drag?.to === index && drag.to > drag.from}
            style:--drag-y={`${drag?.id === point.id ? drag.delta : 0}px`}>
            <button type="button" class="stop" class:highlighted={hoveredId === point.id} onclick={() => onInspect(point)}
                onmouseenter={() => onHover?.(point.id)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(point.id)} onblur={() => onHover?.(null)}>
                <Icon name={point.kind === 'start' || point.kind === 'finish' ? 'pin' : 'flag'} size={17} />
                <span>
                    <strong>{point.label}</strong>
                    {#if measured}<small>{distance.toFixed(1)} km</small>{/if}
                </span>
            </button>
            {#if index > 0 && index < listed.length - 1}
                <button type="button" class="handle" data-point={point.id} aria-label={`Reorder ${point.label}`} aria-describedby={helpId}
                    title="Drag to reorder · use ↑ or ↓ when focused" disabled={listed.length < 4}
                    onpointerdown={(event) => startDrag(event, point.id, index)} onpointermove={dragMove}
                    onpointerup={drop} onpointercancel={() => drag = null}
                    onkeydown={(event) => {
                        if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
                            event.preventDefault();
                            move(point.id, index, index + (event.key === 'ArrowUp' ? -1 : 1), true);
                        }
                    }}><Icon name="grip" size={20} /></button>
            {:else}
                <small class="end">{point.kind === 'start' ? 'Start' : 'Finish'}</small>
            {/if}
        </li>
    {/each}
</ol>
{#if measured}<p class="note">Cumulative from the start</p>{/if}
{#if stops.some(stop => stop.point.kind === 'via' || stop.point.leg === 'drawn')}<p class="note">Edit shaping points on the map. Reordering stops clears shapes on changed legs. Undo restores them.</p>{/if}
<span class="sr-only" id={helpId}>Drag to reorder. Use the up and down arrow keys when focused.</span>
<span class="sr-only" role="status">{announcement}</span>

<style>
    .route {
        margin: 0;
        padding: 4px 16px 0;
        list-style: none;
    }
    li {
        position: relative;
        display: flex;
        align-items: center;
        gap: 8px;
        margin-bottom: 4px;
    }
    li.dragging { z-index: 1; transform: translateY(var(--drag-y)); background: var(--panel); border-radius: 6px; box-shadow: var(--planner-shadow); pointer-events: none; }
    li.drop-before::before, li.drop-after::after { content: ''; position: absolute; left: 0; right: 0; height: 2px; background: var(--forest); }
    li.drop-before::before { top: -3px; }
    li.drop-after::after { bottom: -3px; }
    button {
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        cursor: pointer;
    }
    .stop {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        gap: 12px;
        margin-left: -8px;
        padding: 8px;
        border-radius: 6px;
        text-align: left;
    }
    .stop:hover, .stop.highlighted {
        background: var(--parchment-2);
    }
    .stop > :global(svg) {
        flex: none;
        color: var(--ink-soft);
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
        font-size: 13px;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
    .end {
        margin: 0;
    }
    .handle {
        display: grid;
        place-items: center;
        width: 32px;
        height: 36px;
        border-radius: 6px;
        color: var(--ink-soft);
        cursor: grab;
        touch-action: none;
    }
    .handle:active { cursor: grabbing; }
    .handle:hover:not(:disabled) {
        background: var(--parchment-2);
    }
    .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .note {
        margin: 8px 16px 16px;
        font-size: 13px;
        color: var(--ink-soft);
    }
</style>
