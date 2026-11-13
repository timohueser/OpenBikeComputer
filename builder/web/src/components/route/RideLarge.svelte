<script lang="ts">
    import { untrack } from 'svelte';
    import Window from '../../lib/ui/Window.svelte';
    import Lanes from './Lanes.svelte';
    import Plot from './Plot.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import ZoneTimes from './ZoneTimes.svelte';
    import { fitView, range, viewColumns } from '../../lib/ride/lines';
    import type { Channel } from '../../lib/ride/numbers';
    import { formatAlong, formatDistance } from '../../lib/settings/format';
    import type { Zone } from '../../lib/settings/zones';

    /** One line of a ride large in a window, on the distance axis. A line with zones has the colour of the zone of each
     * stretch, with the time in each zone beside it; the elevation has the grade colours of the Route page; any other
     * line keeps its colour and has its scale after it. The pointer shows the value at a place; it does not move the
     * map. The window shows a stretch of the line, its view: a drag across the line makes that stretch the view, a pinch
     * on a trackpad changes its length about the pointer, and a sideways scroll moves it. The ticks, the scale, the
     * numbers and the zone times are of the view. "Whole ride" beside the title, Escape or a double click shows the
     * whole line again. The window opens on a selected part; in a longer view the part shows as in the rows, and a
     * click on it makes it the view. */
    let { title, color, format, xs, lengthM, values, of, unit = '', missing = '', grades = false, part, start, onClose }: {
        title: string;
        color: string;
        format: (value: number) => string;
        /** The place of each sample on the axis from 0 to 1, and the length of the axis. */
        xs: number[];
        lengthM: number;
        /** The smoothed value of each sample. */
        values: (number | undefined)[];
        /** The zones of the line, its sensor for the time in each zone and the numbers under the line: of a stretch of
         * the axis, else of the whole ride. */
        of: (stretch: [number, number] | null) => { zones?: Zone[]; times?: Channel; numbers: { label: string; value: string }[] };
        /** The unit of the zones, and the limit that they need (`ZoneTimes`). */
        unit?: string;
        missing?: string;
        /** The line is the elevation. */
        grades?: boolean;
        /** The selected part on the axis. */
        part: [number, number] | null;
        /** The name of the start of the axis. */
        start: string;
        onClose: () => void;
    } = $props();

    /** The line is PLOT_PX high, and LOW_PLOT_PX in a window up to NARROW_PX wide. */
    const PLOT_PX = 280, LOW_PLOT_PX = 200, NARROW_PX = 760;
    /** The view is never shorter than MIN_M. A drag shorter than DRAG_PX is no stretch. The view moves to a new stretch
     * in ZOOM_MS, so that the eye sees where the stretch was; the numbers follow NUMBERS_MS after its last move. A pinch
     * changes the length of the view by PINCH for each pixel. The scale of a view is never less than LEAST_RANGE of the
     * scale of the whole line, so a flat stretch stays flat. */
    const MIN_M = 200, DRAG_PX = 12, ZOOM_MS = 220, NUMBERS_MS = 150, PINCH = .01, LEAST_RANGE = .25;
    type Stretch = [number, number];
    const WHOLE: Stretch = [0, 1];
    let width = $state(0), innerWidth = $state(1280), pointer = $state<number>(), frame = $state<HTMLElement>();
    /** The stretch of the axis in view; the stretch that the numbers are of; the stretch of a drag, in the view. */
    let view = $state.raw<Stretch>(untrack(() => part ? fitView(part, MIN_M / lengthM) : WHOLE));
    let counted = $state.raw<Stretch>(untrack(() => view));
    let drag = $state.raw<Stretch | null>(null);

    const least = $derived(MIN_M / lengthM);
    const length = $derived(view[1] - view[0]);
    const zoomed = $derived(view[0] > 0 || view[1] < 1);
    const columns = $derived(viewColumns(xs, values, view, Math.max(1, Math.round(width))));
    const span = $derived.by(() => {
        const whole = range(values) ?? { low: 0, high: 1 }, here = zoomed ? range(columns) : undefined;
        if (!here) return whole;
        const room = Math.max(here.high - here.low, (whole.high - whole.low) * LEAST_RANGE);
        const low = Math.min(whole.high - room, Math.max(whole.low, (here.low + here.high - room) / 2));
        return { low, high: low + room };
    });
    const ticks = $derived(fitTicks([
        { at: 0, label: view[0] > 0 ? formatAlong(view[0] * lengthM) : start },
        { at: 1, label: view[1] < 1 ? formatAlong(view[1] * lengthM) : formatDistance(lengthM) },
        ...distanceTicks(view[0] * lengthM, view[1] * lengthM, width, true),
    ], width));
    /** The pale layers: the stretch of a drag, else the selected part where the view shows more than the part. */
    const band = $derived.by((): Stretch | null => {
        if (drag) return drag;
        if (!part) return null;
        const at = (x: number) => Math.min(1, Math.max(0, (x - view[0]) / length)), shown: Stretch = [at(part[0]), at(part[1])];
        return shown[1] - shown[0] >= 1 || shown[1] === shown[0] ? null : shown;
    });
    const all = (stretch: Stretch) => stretch[0] <= 0 && stretch[1] >= 1;
    const shown = $derived(of(all(counted) ? null : counted));
    const stretchText = $derived(all(counted) ? ''
        : `${formatAlong(counted[0] * lengthM)} to ${formatAlong(counted[1] * lengthM)} · ${formatDistance((counted[1] - counted[0]) * lengthM)}`);
    const rows = $derived([{
        label: '', draw: plot, height: innerWidth > NARROW_PX ? PLOT_PX : LOW_PLOT_PX,
        // The rules of the zones are the scale of a line with zones; the numbers under the elevation are its scale.
        scale: shown.zones || grades ? undefined : [format(span.high), format(span.low)] as [string, string],
    }]);

    $effect(() => {
        const next = view, timer = setTimeout(() => { counted = next; }, NUMBERS_MS);
        return () => clearTimeout(timer);
    });

    /** The stretch that the view moves to: Escape closes the window while the view is on its way to the whole line. */
    let moving = 0, aim = untrack(() => view);
    function show(to: Stretch) {
        cancelAnimationFrame(moving);
        aim = to;
        const from = view, began = performance.now();
        if (matchMedia('(prefers-reduced-motion: reduce)').matches) return void (view = to);
        const step = (now: number) => {
            const t = Math.min(1, (now - began) / ZOOM_MS), eased = 1 - (1 - t) ** 3;
            view = t === 1 ? to : [from[0] + (to[0] - from[0]) * eased, from[1] + (to[1] - from[1]) * eased];
            if (t < 1) moving = requestAnimationFrame(step);
        };
        moving = requestAnimationFrame(step);
    }
    /** The button leaves with the view, so the focus goes to the line. */
    function showWhole() {
        show(WHOLE);
        frame?.querySelector<HTMLElement>('[role="slider"]')?.focus();
    }

    /** The end of a drag: its stretch becomes the view. */
    function release() {
        const stretch = drag;
        drag = null;
        if (stretch && (stretch[1] - stretch[0]) * width >= DRAG_PX) show(fitView([view[0] + stretch[0] * length, view[0] + stretch[1] * length], least));
    }
    /** A click on the selected part shows the part; Enter shows the stretch that Shift with the arrow keys made. */
    function click(x: number) {
        if (drag) release();
        else if (part && band && x >= band[0] && x <= band[1]) show(fitView(part, least));
    }
    // In the capture phase and with no default, so that the window stays open.
    function escape(event: KeyboardEvent) {
        if (event.key !== 'Escape' || all(aim)) return;
        event.preventDefault();
        showWhole();
    }

    // A pinch on a trackpad arrives as a wheel with Ctrl. A scroll up or down stays the scroll of the window. The
    // listener is not passive, so that the browser neither zooms the page nor goes back in its history.
    $effect(() => {
        const target = frame;
        if (!target) return;
        const wheel = (event: WheelEvent) => {
            const box = target.querySelector('.lane')!.getBoundingClientRect(), sideways = Math.abs(event.deltaX) > Math.abs(event.deltaY);
            if (!event.ctrlKey && !(zoomed && sideways)) return;
            event.preventDefault();
            cancelAnimationFrame(moving);
            if (event.ctrlKey) {
                // The place under the pointer stays under the pointer.
                const x = Math.min(1, Math.max(0, (event.clientX - box.left) / box.width));
                const next = Math.min(1, Math.max(least, length * Math.exp(event.deltaY * PINCH))), from = view[0] + x * (length - next);
                view = fitView([from, from + next], least);
            } else {
                const from = view[0] + event.deltaX / box.width * length;
                view = fitView([from, from + length], least);
            }
            aim = view;
        };
        target.addEventListener('wheel', wheel, { passive: false });
        return () => target.removeEventListener('wheel', wheel);
    });
</script>

<svelte:window bind:innerWidth onkeydowncapture={escape} />

{#snippet plot()}
    <div class="plot">
        <Plot {columns} low={span.low} high={span.high} {color} area={grades} zones={shown.zones} grades={grades ? length * lengthM : undefined} mark={pointer} {format} bind:width />
    </div>
{/snippet}

{#snippet chip()}{pointer === undefined ? '' : formatDistance((view[0] + pointer * length) * lengthM)}{/snippet}

{#snippet beside()}
    {#if zoomed}
        <button type="button" class="whole" onclick={showWhole}>
            <svg viewBox="0 0 14 14" aria-hidden="true"><path d="M1.5 3v8M12.5 3v8M4 7h6M6 5 4 7l2 2M8 5l2 2-2 2" /></svg>Whole ride
        </button>
    {/if}
{/snippet}

<Window {title} wide {onClose} {beside}>
    {#snippet children(close)}
        <div class="body" class:beside={shown.times?.zones}>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="view" bind:this={frame} onpointerup={release} onpointercancel={() => drag = null} ondblclick={showWhole}>
                <Lanes label={title} {rows} {ticks} {pointer} {chip} part={band} onpart={next => drag = next} onclick={click} onpointer={x => pointer = x ?? undefined} />
            </div>
            {#if shown.times?.zones}
                <section>
                    <h3>Time in each zone</h3>
                    <ZoneTimes channel={shown.times} {unit} {missing} list />
                </section>
            {/if}
        </div>
        <div class="foot">
            <dl>
                {#each shown.numbers as number (number.label)}<div><dt>{number.label}</dt><dd>{number.value}</dd></div>{/each}
            </dl>
            {#if stretchText}<p>{stretchText}</p>{/if}
        </div>
        <!-- The link opens the Settings, which are a window too: this window closes first. -->
        {#if shown.times && !shown.times.zones && missing}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="limit" onclick={event => { if ((event.target as Element).closest('a')) close(); }}><ZoneTimes channel={shown.times} {unit} {missing} /></div>
        {/if}
    {/snippet}
</Window>

<style>
    /* The way back is a button of the pages, at the size of the title row. It comes in from the title, so the eye
       finds it at the end of a drag. */
    .whole {
        display: inline-flex;
        gap: 6px;
        align-items: center;
        padding: 4px 10px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        color: var(--ink);
        font: 13px/16px var(--sans);
        white-space: nowrap;
    }
    .whole:hover {
        background: var(--parchment-2);
    }
    .whole svg {
        width: 14px;
        height: 14px;
        fill: none;
        stroke: currentColor;
        stroke-width: 1.4;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    @media (prefers-reduced-motion: no-preference) {
        .whole {
            animation: arrive 200ms cubic-bezier(.2, .8, .2, 1);
        }
    }
    @keyframes arrive {
        from {
            opacity: 0;
            translate: -8px 0;
        }
    }
    .body {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 20px 28px;
    }
    .beside {
        grid-template-columns: minmax(0, 1fr) 200px;
    }
    .view :global(.lane) {
        cursor: crosshair;
    }
    .plot {
        height: 100%;
        --plot-stroke: 2;
    }
    h3 {
        margin: 0 0 10px;
        color: var(--ink-soft);
        font: 600 13px/1.3 var(--sans);
    }
    /* The stretch stands on the line of the figures, so it comes and goes with no move of the numbers. */
    .foot {
        display: flex;
        flex-wrap: wrap;
        gap: 6px 40px;
        align-items: flex-end;
        margin-top: 18px;
    }
    dl {
        display: flex;
        gap: 40px;
        margin: 0;
    }
    dt {
        color: var(--ink-soft);
        font: 13px/1.3 var(--sans);
    }
    dd {
        margin: 1px 0 0;
        font: 600 21px/1.2 var(--sans);
        font-variant-numeric: tabular-nums;
    }
    .foot p {
        margin: 0 0 3px;
        color: var(--ink-soft);
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .limit {
        margin-top: 12px;
    }
    /* The zones stand under the line. */
    @media (max-width: 760px) {
        .beside {
            grid-template-columns: minmax(0, 1fr);
        }
    }
</style>
