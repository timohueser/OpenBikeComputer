<script lang="ts">
    import type { Snippet } from 'svelte';

    /** Rows on one axis from 0 to 1 under a header, with a narrow column of labels, the pointer as a line across the
     * rows and the ticks under them. With `onpart`, a drag or Shift with an arrow key selects a part of the axis: the
     * rest is paler, the header shows `partText` with Clear, and Escape clears it. On a phone the labels stand above
     * their rows. */
    let { rows, ticks, ticksAfter, header, label, pointer, onpointer, chip, part = null, onpart, partText = '', onclick }: {
        rows: { label: string; draw: Snippet; height?: number }[];
        /** Labels under the rows; the first and the last stay inside the axis. */
        ticks: { at: number; label: string }[];
        /** The ticks follow this many rows; by default they follow every row. */
        ticksAfter?: number;
        /** The controls at the start of the header. */
        header?: Snippet;
        /** The name of the axis for the keys, such as "Profile". */
        label: string;
        pointer?: number;
        /** The place of the pointer on the axis, or null when it leaves the rows; `row` is the index of the row under
         * it, null when the keys move it. */
        onpointer: (x: number | null, row?: number | null) => void;
        /** The text at the foot of the pointer line, over the ticks. */
        chip?: Snippet;
        part?: [number, number] | null;
        onpart?: (part: [number, number] | null) => void;
        partText?: string;
        /** A click or a tap with no drag, or Enter; `row` as for `onpointer`. Without it, a click clears the part. */
        onclick?: (x: number, row?: number | null) => void;
    } = $props();

    /** A drag starts after DRAG_PX; an arrow key moves the pointer by KEY_STEP of the axis. */
    const DRAG_PX = 4, KEY_STEP = .01;
    let root = $state<HTMLElement>(), axis = $state<HTMLElement>(), tickRow = $state<HTMLElement>(), lineEnd = $state<number>();
    let down: { x: number; px: number; row?: number; moved: boolean } | null = null;
    const split = $derived(Math.min(rows.length, ticksAfter ?? rows.length));
    const gridRow = (i: number) => i < split ? i + 1 : i + 2;

    // With rows under the ticks, the pointer line ends on the ticks, where its chip shows. The gaps differ on a phone,
    // so the place is measured.
    $effect(() => {
        if (!root || !axis || !tickRow || split === rows.length) return void (lineEnd = undefined);
        const [box, ticksBox, measure] = [axis, tickRow, () => { lineEnd = ticksBox.offsetTop - box.offsetTop; }];
        const observer = new ResizeObserver(measure);
        observer.observe(root);
        return () => observer.disconnect();
    });

    function place(event: PointerEvent) {
        const box = axis!.getBoundingClientRect(), lane = (event.target as Element).closest<HTMLElement>('.lane');
        return { x: (event.clientX - box.left) / box.width, px: event.clientX, row: lane ? Number(lane.dataset.row) : undefined };
    }

    function move(event: PointerEvent) {
        const { x, px, row } = place(event), clamped = Math.min(1, Math.max(0, x));
        if (down && onpart && (down.moved || Math.abs(px - down.px) >= DRAG_PX)) {
            down.moved = true;
            onpart([Math.min(down.x, clamped), Math.max(down.x, clamped)]);
        }
        if (down) onpointer(clamped, down.row);
        else onpointer(x >= 0 && x <= 1 ? x : null, row);
    }

    function start(event: PointerEvent) {
        const { x, px, row } = place(event);
        // A button, a link or a panel (`data-panel`) in a lane keeps its own press.
        if (!onpart || x < 0 || x > 1 || event.button || (event.target as Element).closest('button, a, [data-panel]')) return;
        down = { x, px, row, moved: false };
        (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    }

    function end() {
        if (down && !down.moved) {
            if (onclick) onclick(down.x, down.row);
            else onpart?.(null);
        }
        down = null;
    }

    function key(event: KeyboardEvent) {
        const at = pointer ?? 0, step = event.key === 'ArrowRight' ? KEY_STEP : event.key === 'ArrowLeft' ? -KEY_STEP : 0;
        if (event.key === 'Enter') onclick?.(at, null);
        if (!step) return;
        event.preventDefault();
        const next = Math.min(1, Math.max(0, at + step));
        if (event.shiftKey && onpart) {
            // The end of the part away from the pointer stays.
            const anchor = !part ? at : Math.abs(part[1] - at) < Math.abs(part[0] - at) ? part[0] : part[1];
            onpart([Math.min(anchor, next), Math.max(anchor, next)]);
        }
        onpointer(next, null);
    }

    // A menu or a panel that Escape closes first prevents the default, in the capture phase.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || event.defaultPrevented || !part || !onpart) return;
        if (!(event.target as Element).closest?.('input, textarea, dialog')) onpart(null);
    }
</script>

<svelte:window onkeydown={escape} />

{#if header || part}
    <header>
        {@render header?.()}
        {#if part && partText}
            <p class="part">{partText}<button type="button" class="clear" onclick={() => onpart?.(null)}>Clear</button></p>
        {/if}
    </header>
{/if}
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="lanes" bind:this={root} onpointermove={move} onpointerdown={start} onpointerup={end} onpointercancel={() => down = null}
    onpointerleave={() => { if (!down) onpointer(null); }}>
    {#each rows as row, i (i)}
        <div class="label" style:grid-row={gridRow(i)} style:--row={row.height ? `${row.height}px` : undefined}>{row.label}</div>
        <div class="lane" data-row={i} style:grid-row={gridRow(i)} style:height={row.height ? `${row.height}px` : undefined}>{@render row.draw()}</div>
    {/each}
    <!-- Each part has its grid row: the pointer layer covers the rows of the lanes. -->
    <div class="ticks" bind:this={tickRow} style:grid-row={split + 1}>
        {#each ticks as tick, i (i)}<span style:left={`${tick.at * 100}%`} style:translate={`${-tick.at * 100}%`}>{tick.label}</span>{/each}
    </div>
    <!-- The pointer layer is the slider of the keys: it holds no button. Shift with an arrow grows a part, Enter clicks
         at the pointer. -->
    <div class="over" bind:this={axis} style:grid-row={`1 / span ${split < rows.length ? rows.length + 1 : rows.length}`}
        role="slider" tabindex="0" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round((pointer ?? 0) * 100)}
        onkeydown={key} onblur={() => onpointer(null)}>
        {#if part}
            <b class="pale" style:left="0" style:width={`${part[0] * 100}%`}></b>
            <b class="pale" style:left={`${part[1] * 100}%`} style:right="0"></b>
        {/if}
        {#if pointer !== undefined}
            <i style:left={`${pointer * 100}%`} style:bottom={lineEnd === undefined ? undefined : 'auto'}
                style:height={lineEnd === undefined ? undefined : `${lineEnd + 4}px`}>
                {#if chip}<span class="chip" class:flip={pointer > .7}>{@render chip()}</span>{/if}
            </i>
        {/if}
    </div>
</div>

<style>
    header {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px 12px;
        margin-bottom: 10px;
    }
    .part {
        display: flex;
        align-items: baseline;
        gap: 10px;
        margin: 0 0 0 4px;
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .clear {
        padding: 0;
        border: 0;
        background: none;
        color: var(--ink-soft);
        font-size: 13px;
        text-decoration: underline;
    }
    .clear:hover {
        color: var(--ink);
    }
    .lanes {
        position: relative;
        display: grid;
        grid-template-columns: 72px minmax(0, 1fr);
        column-gap: 16px;
        row-gap: 6px;
        touch-action: pan-y;
        user-select: none;
        /* How much of the panel covers the rows outside a part. */
        --pale: .7;
    }
    /* On the dark panel the same share leaves too little of the rows to read. */
    :global(:root[data-theme="dark"]) .lanes {
        --pale: .55;
    }
    .over:focus-visible {
        border-radius: 2px;
        outline: 2px solid var(--ink-soft);
        outline-offset: 4px;
    }
    .label {
        grid-column: 1;
        min-width: 0;
        color: var(--ink-soft);
        font-size: 12px;
        /* The label stands at the middle of a low row, such as the pins, and at the top of a high one. */
        line-height: min(var(--row, 16px), 28px);
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
        z-index: 2;
        grid-column: 2;
        pointer-events: none;
    }
    .pale {
        position: absolute;
        top: -4px;
        bottom: -4px;
        background: var(--panel);
        opacity: var(--pale);
    }
    .pale:first-child {
        border-right: 1px solid var(--ink-soft);
    }
    .pale + .pale {
        border-left: 1px solid var(--ink-soft);
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
        z-index: 4;
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
    /* The labels stand above their rows as captions, in the gaps; the plots take the full width. */
    @media (max-width: 760px) {
        .lanes {
            grid-template-columns: 0 minmax(0, 1fr);
            column-gap: 0;
            row-gap: 22px;
            padding-top: 18px;
        }
        .label {
            z-index: 1;
            grid-column: 2;
            align-self: start;
            margin-top: -18px;
            line-height: 16px;
            pointer-events: none;
        }
        .ticks {
            margin-top: -16px;
        }
    }
</style>
