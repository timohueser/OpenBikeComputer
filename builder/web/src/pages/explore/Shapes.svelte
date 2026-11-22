<!-- The shapes that the rider accepts: loop, there and back, and the two lift shapes. A lift shape counts, and shows as
     pressed, only while lifts are on; a click on it turns lifts on. At least one shape that counts stays on. -->
<script lang="ts">
    import { SHAPES, type Query, type Shape } from './search';
    import ShapeIcon from './ShapeIcon.svelte';
    import { SHAPE_WORDS } from './words';

    let { query, onchange }: { query: Query; onchange: (change: Partial<Query>) => void } = $props();

    const lift = (shape: Shape) => shape === 'liftDown' || shape === 'liftUp';
    const counted = $derived(SHAPES.filter(shape => query.lift || !lift(shape)));
    const on = $derived(counted.filter(shape => query.shapes.includes(shape)));

    function toggle(shape: Shape) {
        if (!counted.includes(shape)) return onchange({ lift: 'helps', shapes: SHAPES.filter(one => one === shape || on.includes(one)) });
        const next = query.shapes.includes(shape) ? query.shapes.filter(one => one !== shape) : SHAPES.filter(one => one === shape || query.shapes.includes(one));
        if (next.some(one => counted.includes(one))) onchange({ shapes: next });
    }
</script>

<div class="tiles">
    {#each SHAPES as shape (shape)}
        {@const pressed = on.includes(shape)}
        <button type="button" class="tile" aria-pressed={pressed} title={pressed && on.length === 1 ? 'Keep at least one shape' : undefined}
            onclick={() => toggle(shape)}><ShapeIcon {shape} />{SHAPE_WORDS[shape].label}</button>
    {/each}
</div>

<style>
    .tiles {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 8px;
    }
    .tile {
        display: flex;
        align-items: center;
        gap: 9px;
        min-height: 40px;
        padding: 0 12px;
        border: 1px solid var(--line-strong, var(--line));
        border-radius: 8px;
        background: var(--panel);
        color: var(--ink-soft);
        font: 13.5px/1.2 var(--sans);
        text-align: left;
        transition: transform 160ms cubic-bezier(0.23, 1, 0.32, 1), background 120ms, border-color 120ms;
    }
    .tile :global(svg) {
        flex: none;
    }
    .tile:hover {
        border-color: var(--ink-soft);
    }
    .tile:active {
        transform: scale(0.97);
    }
    .tile[aria-pressed="true"] {
        border-color: var(--ink-soft);
        background: var(--parchment-2);
        color: var(--ink);
        font-weight: 600;
    }
</style>
