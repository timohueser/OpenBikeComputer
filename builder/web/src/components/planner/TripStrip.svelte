<script lang="ts">
    import type { Attachment } from 'svelte/attachments';
    import { dayColor } from '../../lib/planner/day-colors';
    import { duration, type ItineraryDay } from '../../lib/planner/editor';
    import { addDays } from '../../lib/planner/layers/data-layer';
    import { centredOn, end, home, step, type Week } from '../../lib/planner/week';
    import { formatDistance, formatHeight } from '../../lib/settings/format';

    let { itinerary, week, theme, startDate, onMove, dragging = $bindable(false) }: {
        /** The shown days, one for each calendar day. */
        itinerary: ItineraryDay[];
        week: Week;
        theme: 'light' | 'dark';
        startDate?: string;
        /** Moves the week to start at this calendar day. */
        onMove: (start: number) => void;
        /** True while the pointer is down on the strip. */
        dragging?: boolean;
    } = $props();

    const count = $derived(itinerary.length);
    const longest = $derived(Math.max(0, ...itinerary.map(day => day.hours)));
    const inside = (day: number) => day >= week.start && day <= week.end;
    // Each slot and the 2 px gap after it take one share of the width and the last gap.
    const at = (slots: number) => `(100% + 2px) * ${slots / count}`;
    const label = (day: number) => startDate
        ? new Date(addDays(startDate, day - 1)).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' }) : String(day);
    let strip = $state<HTMLDivElement>();
    let hovered = $state<number | null>(null);
    // The day outside the week under the pointer, while the pointer is up.
    const lit = $derived(hovered !== null && !dragging && !inside(hovered) ? itinerary[hovered - 1] : undefined);
    // The days from the start of the week to the day under the pointer while it is down.
    let held = 0;
    let wheeled = 0;

    function go(start: number) {
        if (start !== week.start) onMove(start);
    }

    function dayAt(event: PointerEvent) {
        const box = strip!.getBoundingClientRect();
        return Math.max(1, Math.min(count, Math.floor((event.clientX - box.left) / (box.width + 2) * count) + 1));
    }

    function press(event: PointerEvent) {
        if (event.button !== 0) return;
        strip!.focus();
        strip!.setPointerCapture(event.pointerId);
        const day = dayAt(event);
        if (!inside(day)) go(centredOn(count, day));
        held = day - week.start;
        dragging = true;
    }

    function move(event: PointerEvent) {
        const day = dayAt(event);
        if (dragging) go(step(count, week.start, day - held - week.start));
        else hovered = day;
    }

    function key(event: KeyboardEvent) {
        const by = ({ ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1, PageDown: -7, PageUp: 7 } as Record<string, number>)[event.key];
        const start = by ? step(count, week.start, by) : event.key === 'Home' ? home : event.key === 'End' ? end(count) : null;
        if (start === null) return;
        event.preventDefault();
        go(start);
    }

    // A non-passive listener, so the page does not scroll under the strip.
    const wheel: Attachment<HTMLElement> = node => {
        const scroll = (event: WheelEvent) => {
            event.preventDefault();
            wheeled += Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
            const by = Math.trunc(wheeled / 50);
            wheeled -= by * 50;
            if (by) go(step(count, week.start, by));
        };
        node.addEventListener('wheel', scroll, { passive: false });
        return () => node.removeEventListener('wheel', scroll);
    };
</script>

<div class="strip" role="slider" tabindex="0" aria-label="Week in focus" aria-valuemin={1} aria-valuemax={end(count)} aria-valuenow={week.start}
    aria-valuetext={`Days ${week.start} to ${week.end}`} bind:this={strip} {@attach wheel}
    onpointerdown={press} onpointermove={move} onpointerup={() => dragging = false} onpointercancel={() => dragging = false}
    onpointerleave={() => hovered = null} onkeydown={key}>
    {#each itinerary as day (day.number)}
        <span class="slot" class:lit={lit === day} aria-hidden="true">
            {#if day.rest}<i class="rest"></i>
            {:else}<i class="bar" style:height={`${4 + (longest ? 22 * day.hours / longest : 0)}px`}
                style:--day-color={inside(day.number) || lit === day ? dayColor(day.ridingNumber, theme) : undefined}></i>{/if}
            {#if day.number % 7 === 1}<small>{label(day.number)}</small>{/if}
        </span>
    {/each}
    <span class="frame" style:left={`calc(${at(week.start - 1)} - 2px)`} style:width={`calc(${at(7)} + 2px)`}></span>
    {#if lit}
        <span class="tip" role="tooltip" style:left={`calc(${at(lit.number - .5)} - 1px)`} style:translate={`${-(lit.number - .5) / count * 100}% 0`}>
            {lit.rest ? `Day ${lit.number} · Rest` : `Day ${lit.number} · ${formatDistance(lit.distance * 1000)} · ${duration(lit.hours)} · ↑ ${formatHeight(lit.ascent)}`}
        </span>
    {/if}
</div>

<style>
    .strip {
        position: relative;
        display: flex;
        flex: none;
        gap: 2px;
        height: 40px;
        cursor: pointer;
        touch-action: none;
        user-select: none;
    }
    .strip:focus-visible { outline: none; }
    .strip:focus-visible .frame { outline: 2px solid var(--ink); outline-offset: 1px; }
    .slot {
        position: relative;
        flex: 1;
        min-width: 0;
        height: 28px;
        display: flex;
        align-items: flex-end;
        justify-content: center;
    }
    .bar {
        width: 100%;
        border-radius: 2px;
        background: var(--day-color, color-mix(in srgb, var(--ink-faint) 45%, var(--panel)));
    }
    .lit .bar { outline: 2px solid var(--ink); }
    .rest {
        width: 6px;
        height: 6px;
        margin-bottom: 1px;
        box-sizing: border-box;
        border: 1px solid var(--ink-soft);
    }
    small {
        position: absolute;
        top: 30px;
        left: 0;
        padding-left: 3px;
        border-left: 1px solid var(--line-strong);
        font-size: 10px;
        line-height: 10px;
        color: var(--ink-soft);
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }
    .frame {
        position: absolute;
        top: 0;
        height: 30px;
        box-sizing: border-box;
        border: 2px solid var(--amber);
        border-radius: 5px;
        background: color-mix(in srgb, var(--amber) 9%, transparent);
        pointer-events: none;
    }
    .tip {
        position: absolute;
        z-index: 3;
        bottom: calc(100% + 2px);
        padding: 3px 8px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        color: var(--ink);
        font-size: 12px;
        white-space: nowrap;
        pointer-events: none;
    }
</style>
