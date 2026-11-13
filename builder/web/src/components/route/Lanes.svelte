<script lang="ts">
    import type { Snippet } from 'svelte';

    /** Rows on one axis from 0 to 1 beside a column of labels, with the pointer as a line across the rows and the ticks under them. */
    let { rows, ticks, pointer, onpointer, chip }: {
        rows: { label: string | Snippet; draw: Snippet; height?: number }[];
        /** Labels under the rows; the first and the last stay inside the axis. */
        ticks: { at: number; label: string }[];
        pointer?: number;
        /** The place of the pointer on the axis, or null when it leaves the rows. */
        onpointer: (x: number | null) => void;
        /** The text at the foot of the pointer line, over the ticks. */
        chip?: Snippet;
    } = $props();

    let axis = $state<HTMLElement>();

    function move(event: PointerEvent) {
        const box = axis!.getBoundingClientRect(), x = (event.clientX - box.left) / box.width;
        onpointer(x >= 0 && x <= 1 ? x : null);
    }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="lanes" onpointermove={move} onpointerleave={() => onpointer(null)}>
    {#each rows as row, i (i)}
        <div class="label" style:grid-row={i + 1}>{#if typeof row.label === 'string'}{row.label}{:else}{@render row.label()}{/if}</div>
        <div class="lane" style:grid-row={i + 1} style:height={row.height ? `${row.height}px` : undefined}>{@render row.draw()}</div>
    {/each}
    <!-- Each part has its grid row: the pointer layer covers the rows of the lanes. -->
    <div class="ticks" style:grid-row={rows.length + 1}>
        {#each ticks as tick, i (i)}<span style:left={`${tick.at * 100}%`} style:translate={`${-tick.at * 100}%`}>{tick.label}</span>{/each}
    </div>
    <div class="over" bind:this={axis} style:grid-row={`1 / span ${rows.length}`}>
        {#if pointer !== undefined}
            <i style:left={`${pointer * 100}%`}>{#if chip}<span class="chip" class:flip={pointer > .7}>{@render chip()}</span>{/if}</i>
        {/if}
    </div>
</div>

<style>
    .lanes {
        display: grid;
        grid-template-columns: 124px minmax(0, 1fr);
        column-gap: 16px;
        row-gap: 8px;
        touch-action: pan-y;
    }
    .label {
        display: flex;
        flex-direction: column;
        justify-content: center;
        gap: 6px;
        grid-column: 1;
        min-width: 0;
        color: var(--ink-soft);
        font-size: 13px;
    }
    .lane {
        position: relative;
        grid-column: 2;
        min-width: 0;
    }
    .ticks {
        position: relative;
        grid-column: 2;
        height: 16px;
        color: var(--ink-soft);
        font-size: 12px;
        font-variant-numeric: tabular-nums;
    }
    .ticks span {
        position: absolute;
        white-space: nowrap;
    }
    .over {
        position: relative;
        grid-column: 2;
        pointer-events: none;
    }
    i {
        position: absolute;
        top: -4px;
        bottom: -4px;
        z-index: 3;
        width: 0;
        border-left: 1.5px solid var(--route);
    }
    .chip {
        position: absolute;
        top: calc(100% + 2px);
        left: 0;
        padding: 2px 7px;
        border-radius: 6px;
        background: var(--ink);
        color: var(--panel);
        font: 600 12px/1.4 var(--sans);
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .chip.flip {
        right: 0;
        left: auto;
    }
    @media (max-width: 760px) {
        .lanes {
            grid-template-columns: 76px minmax(0, 1fr);
            column-gap: 10px;
        }
    }
</style>
