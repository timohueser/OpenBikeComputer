<script lang="ts">
    import { tick } from 'svelte';
    import Icon from '../../lib/ui/Icon.svelte';
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
    /** The sign of a row, as the map draws its pin: the start, the finish, a turn, a pass, a marker or a visit. */
    const sign = (point: RoutePoint, index: number) => index === 0 ? 'start' : index === last ? 'finish' : point.turnaround ? 'turn' : point.kind === 'pass' ? 'pass' : point.kind === 'marker' ? 'marker' : 'visit';
    const icons: Record<string, string | undefined> = { start: 'M8 5v14l11-7z', finish: 'M5 22V3m0 1c5-4 9 4 15 0v10c-6 4-10-4-15 0', visit: 'M5 22V3m0 1c5-4 9 4 15 0v10c-6 4-10-4-15 0', turn: 'M7 20V9a5 5 0 0 1 10 0v6m-3-3 3 3 3-3' };
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
        {@const kind = sign(point, index)}
        <li class:dragging={drag?.id === point.id && index < last && Math.abs(drag.delta) > 4}
            class:drop-before={drag?.to === index && drag.to < drag.from}
            class:drop-after={drag?.to === index && drag.to > drag.from}
            style:--drag-y={`${drag?.id === point.id && index < last ? drag.delta : 0}px`}>
            <button type="button" class="stop" class:highlighted={hoveredId === point.id} onclick={() => onInspect(point)}
                onmouseenter={() => onHover?.(point.id)} onmouseleave={() => onHover?.(null)} onfocus={() => onHover?.(point.id)} onblur={() => onHover?.(null)}>
                <span class={['sign', kind]}>{#if icons[kind]}<Icon path={icons[kind]} size={kind === 'start' ? 12 : 13} />{/if}</span>
                <span class="name"><strong>{point.label}</strong>{#if kind === 'turn'}<small>Turns back here</small>{/if}</span>
                <small class="along">{index === 0 ? 'Start' : measured ? formatAlong(distance * 1000) : index === last ? 'Finish' : ''}</small>
            </button>
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
        </li>
        {#if index === 0 && last === 1}
            <li class="hint"><span class="sign add"><Icon name="plus" size={13} /></span><span class="name"><strong>Add a stop</strong><small>Click the map, or search</small></span></li>
        {/if}
    {/each}
</ol>
<span class="sr-only" id={helpId}>Drag to reorder. Use the up and down arrow keys when focused.</span>
<span class="sr-only" role="status">{announcement}</span>

<style>
    /* The route as a straight line in its colour, with the stops on it. A row carries the sign of its pin on the map. */
    .route {
        margin: 0;
        padding: 4px 12px 16px;
        list-style: none;
    }
    li {
        position: relative;
        display: grid;
        grid-template-columns: minmax(0, 1fr) 24px;
        align-items: center;
        min-height: 48px;
        border-radius: 10px;
    }
    li::before {
        content: "";
        position: absolute;
        left: 22px;
        top: 0;
        bottom: 0;
        z-index: 1;
        width: 4px;
        background: var(--route);
        pointer-events: none;
    }
    li:first-child::before { top: 50%; }
    li:last-child::before { bottom: 50%; }
    li.dragging { z-index: 1; transform: translateY(var(--drag-y)); background: var(--panel); box-shadow: var(--planner-shadow); pointer-events: none; }
    li.dragging::before { display: none; }
    li.drop-before::after, li.drop-after::after { content: ''; position: absolute; left: 0; right: 0; height: 2px; background: var(--forest); }
    li.drop-before::after { top: -1px; }
    li.drop-after::after { bottom: -1px; }
    button {
        border: 0;
        background: none;
        color: var(--ink);
        font: inherit;
        cursor: pointer;
    }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
    .stop, .hint {
        position: relative;
        display: grid;
        grid-template-columns: 28px minmax(0, 1fr) auto;
        column-gap: 12px;
        align-items: center;
        min-height: 48px;
        padding: 2px 4px 2px 10px;
        border-radius: 10px;
        text-align: left;
    }
    .hint { grid-column: 1 / -1; }
    .stop:hover, .stop.highlighted { background: var(--parchment-2); }
    .sign {
        position: relative;
        z-index: 2;
        justify-self: center;
        width: 26px;
        height: 26px;
        display: grid;
        place-items: center;
        box-sizing: border-box;
        border-radius: 50%;
    }
    .start, .finish { border: 1.5px solid var(--ink-soft); background: var(--panel); }
    .finish { border-radius: 7px; }
    .visit, .turn { background: var(--ink-soft); color: var(--panel); box-shadow: 0 0 0 2px var(--panel); }
    .pass { width: 20px; height: 20px; border: 3px solid var(--route); background: var(--panel); box-shadow: 0 0 0 2px var(--panel); }
    .marker { width: 12px; height: 12px; border: 1.5px solid var(--ink-soft); background: var(--parchment-2); box-shadow: 0 0 0 2px var(--panel); }
    .add { border: 1.5px dashed var(--line-strong); background: var(--panel); color: var(--ink-soft); }
    strong, small { display: block; }
    strong { font: 600 14px/1.3 var(--sans); overflow-wrap: anywhere; }
    .hint strong { font-weight: 500; color: var(--ink-soft); }
    small {
        font-size: 13px;
        color: var(--ink-soft);
    }
    .along {
        font-variant-numeric: tabular-nums;
        text-align: right;
        white-space: nowrap;
    }
    .handle {
        display: grid;
        place-items: center;
        width: 24px;
        height: 36px;
        padding: 0;
        border-radius: 6px;
        color: var(--ink-faint);
        cursor: grab;
        touch-action: none;
    }
    .handle:active { cursor: grabbing; }
    .handle:hover:not(:disabled) { background: var(--parchment-2); color: var(--ink-soft); }
    .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
</style>
