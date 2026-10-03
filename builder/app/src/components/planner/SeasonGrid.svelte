<script lang="ts">
    import { COLUMNS, columnDays, monthColumns, seasonColumnDays, seasonMonthColumns, type SeasonGrid } from '../../lib/planner/layers/data-layer';

    let { grid, label, rowHeight = 8, onPick, slider }: {
        grid: SeasonGrid;
        label: string;
        rowHeight?: number;
        /** Makes the grid a click and drag target for a column. */
        onPick?: (column: number) => void;
        /** With `onPick`, makes the grid a slider with a thumb at the marker. */
        slider?: { valueText: string; onKey: (event: KeyboardEvent) => void };
    } = $props();

    let canvas: HTMLCanvasElement;
    let width = $state(0);
    let dragging = $state(false);
    const gap = $derived(grid.rows.length > 1 ? Math.min(3, Math.ceil(rowHeight / 3)) : 0);
    const height = $derived(grid.rows.length * (rowHeight + gap) - gap);
    const labelled = $derived(grid.rows.some(row => row.label));
    // Every few rows carry a label, counted from the newest.
    const every = $derived(Math.ceil(14 / (rowHeight + gap)));
    const days = $derived(grid.seasonal ? seasonColumnDays : columnDays);
    const monthName = (column: number) => days[column].toLocaleDateString('en-GB', { month: width > 480 ? 'short' : 'narrow', timeZone: 'UTC' });

    function hatch(context: CanvasRenderingContext2D, color: string, scale: number) {
        const tile = document.createElement('canvas');
        tile.width = tile.height = 6 * scale;
        const pen = tile.getContext('2d')!;
        pen.strokeStyle = color;
        pen.lineWidth = 1.5 * scale;
        pen.beginPath();
        pen.moveTo(0, 6 * scale); pen.lineTo(6 * scale, 0);
        pen.moveTo(-3 * scale, 3 * scale); pen.lineTo(3 * scale, -3 * scale);
        pen.moveTo(3 * scale, 9 * scale); pen.lineTo(9 * scale, 3 * scale);
        pen.stroke();
        return context.createPattern(tile, 'repeat')!;
    }

    $effect(() => {
        const context = canvas?.getContext('2d');
        if (!context || !width) return;
        const scale = devicePixelRatio || 1;
        canvas.width = Math.round(width * scale);
        canvas.height = Math.round(height * scale);
        context.clearRect(0, 0, canvas.width, canvas.height);
        const fills = grid.swatches.map(swatch => swatch.hatch ? hatch(context, swatch.color, scale) : swatch.color);
        const x = (column: number) => Math.round(column / COLUMNS * canvas.width);
        grid.rows.forEach((row, r) => {
            const top = Math.round(r * (rowHeight + gap) * scale), bottom = Math.round((r * (rowHeight + gap) + rowHeight) * scale);
            for (let start = 0, end = 1; start < COLUMNS; start = end++) {
                while (end < COLUMNS && row.cells[end] === row.cells[start]) end++;
                const fill = fills[row.cells[start]];
                if (!fill) continue;
                context.fillStyle = fill;
                context.fillRect(x(start), top, x(end) - x(start), bottom - top);
            }
        });
    });

    function pick(event: PointerEvent) {
        if (!onPick || (event.type === 'pointermove' && !event.buttons)) return;
        const box = canvas.getBoundingClientRect();
        onPick(Math.max(0, Math.min(COLUMNS - 1, Math.floor((event.clientX - box.left) / box.width * COLUMNS))));
    }
</script>

<div class="season-grid" class:labelled class:pickable={!!onPick} class:slider={!!slider} class:dragging>
    {#if labelled}
        <ol class="rows" aria-hidden="true" style:height={`${height}px`}>
            {#each grid.rows as row, r (row.label)}
                {#if (grid.rows.length - 1 - r) % every === 0}<li style:top={`${r * (rowHeight + gap) + rowHeight / 2}px`}>{row.label}</li>{/if}
            {/each}
        </ol>
    {/if}
    {#snippet plot()}
        <canvas bind:this={canvas} class:outlined={grid.rows.length === 1} aria-hidden="true" style:height={`${height}px`}
            onpointerdown={(event) => { if (onPick) { canvas.setPointerCapture(event.pointerId); dragging = true; pick(event); } }} onpointermove={pick}
            onpointerup={() => dragging = false} onpointercancel={() => dragging = false}></canvas>
        <i class={slider ? 'thumb' : 'marker'} style:left={`${(grid.marker + 0.5) / COLUMNS * 100}%`}></i>
        <ol class="months" aria-hidden="true">
            {#each grid.seasonal ? seasonMonthColumns : monthColumns as column (column)}<li style:left={`${column / COLUMNS * 100}%`}>{monthName(column)}</li>{/each}
        </ol>
    {/snippet}
    {#if slider}
        <div class="plot" role="slider" tabindex="0" aria-label={label} aria-valuemin={0} aria-valuemax={COLUMNS - 1} aria-valuenow={grid.marker}
            aria-valuetext={slider.valueText} bind:clientWidth={width} onkeydown={slider.onKey}>{@render plot()}</div>
    {:else}
        <div class="plot" role="img" aria-label={label} bind:clientWidth={width}>{@render plot()}</div>
    {/if}
</div>

<style>
    .season-grid { display: grid; grid-template-columns: minmax(0, 1fr); }
    .season-grid.labelled { grid-template-columns: 48px minmax(0, 1fr); }
    .rows { position: relative; margin: 0; padding: 0; list-style: none; }
    .rows li { position: absolute; right: 6px; transform: translateY(-50%); font-size: 11px; line-height: 1; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .plot { position: relative; min-width: 0; padding-bottom: 16px; }
    canvas { display: block; width: 100%; }
    /* A single row can be all white snow, so it keeps an outline. */
    canvas.outlined { border-radius: 3px; box-shadow: 0 0 0 1px var(--line-strong); }
    .pickable canvas { cursor: pointer; touch-action: none; }
    .marker { position: absolute; top: -4px; bottom: 12px; width: 2px; transform: translateX(-50%); background: var(--ink); pointer-events: none; }
    /* A slider reads as a track with a handle: the empty track shows, and the thumb stands above and below it. */
    .slider canvas { background: var(--parchment-2); cursor: grab; }
    .slider.dragging canvas { cursor: grabbing; }
    .slider .plot { border-radius: 4px; }
    .slider .plot:focus-visible { outline: 2px solid var(--forest); outline-offset: 4px; }
    .thumb { position: absolute; top: -5px; bottom: 11px; width: 12px; transform: translateX(-50%); border: 2px solid var(--ink); border-radius: 4px; background: var(--panel); box-shadow: 0 1px 3px rgba(28, 27, 20, .3); pointer-events: none; }
    .thumb::after { content: ""; position: absolute; top: 4px; bottom: 4px; left: 50%; width: 2px; transform: translateX(-50%); background: var(--ink); }
    .slider:hover .thumb { background: var(--parchment-2); }
    .months { position: absolute; left: 0; right: 0; bottom: 0; height: 14px; margin: 0; padding: 0; list-style: none; }
    .months li { position: absolute; padding-left: 3px; border-left: 1px solid var(--line-strong); font-size: 11px; line-height: 14px; color: var(--ink-soft); }
    .slider .months { height: 16px; }
    .slider .months li { line-height: 16px; border-left-color: var(--ink-soft); }
</style>
