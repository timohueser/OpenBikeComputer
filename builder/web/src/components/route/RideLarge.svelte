<script lang="ts">
    import Window from '../../lib/ui/Window.svelte';
    import Lanes from './Lanes.svelte';
    import Plot from './Plot.svelte';
    import { distanceTicks, fitTicks } from './ticks';
    import ZoneTimes from './ZoneTimes.svelte';
    import { range, thin } from '../../lib/ride/lines';
    import type { Channel } from '../../lib/ride/numbers';
    import { formatDistance } from '../../lib/settings/format';
    import type { Zone } from '../../lib/settings/zones';

    /** One line of a ride large in a window, on the distance axis of the rows. A line with zones has the colour of the
     * zone of each stretch, with the time in each zone beside it; the elevation has the grade colours of the Route page;
     * any other line keeps its colour and has its scale after it. The pointer shows the value at a place; it does not
     * move the map. A selected part shows as in the rows, and no drag changes it here. */
    let { title, color, format, xs, lengthM, values, zones, times, unit = '', missing = '', grades = false, part, partText, numbers, start, onClose }: {
        title: string;
        color: string;
        format: (value: number) => string;
        /** The place of each sample on the axis from 0 to 1, and the length of the axis. */
        xs: number[];
        lengthM: number;
        /** The smoothed value of each sample. */
        values: (number | undefined)[];
        /** The zones of the line. */
        zones?: Zone[];
        /** The sensor of the line, for the time in each zone: of the selected part, else of the whole ride. */
        times?: Channel;
        /** The unit of the zones, and the limit that they need (`ZoneTimes`). */
        unit?: string;
        missing?: string;
        /** The line is the elevation. */
        grades?: boolean;
        /** The selected part on the axis, and its range in words. */
        part: [number, number] | null;
        partText: string;
        /** The numbers under the line: of the selected part, else of the whole ride. */
        numbers: { label: string; value: string }[];
        /** The name of the start of the axis. */
        start: string;
        onClose: () => void;
    } = $props();

    /** The line is PLOT_PX high, and LOW_PLOT_PX in a window up to NARROW_PX wide. */
    const PLOT_PX = 280, LOW_PLOT_PX = 200, NARROW_PX = 760;
    let width = $state(0), innerWidth = $state(1280), pointer = $state<number>();

    const columns = $derived(thin(xs, values, Math.max(1, Math.round(width))));
    const span = $derived(range(values) ?? { low: 0, high: 1 });
    const ticks = $derived(fitTicks([{ at: 0, label: start }, { at: 1, label: formatDistance(lengthM) }, ...distanceTicks(0, lengthM, width)], width));
    const rows = $derived([{
        label: '', draw: plot, height: innerWidth > NARROW_PX ? PLOT_PX : LOW_PLOT_PX,
        // The rules of the zones are the scale of a line with zones; the numbers under the elevation are its scale.
        scale: zones || grades ? undefined : [format(span.high), format(span.low)] as [string, string],
    }]);
</script>

<svelte:window bind:innerWidth />

{#snippet plot()}
    <div class="plot">
        <Plot {columns} low={span.low} high={span.high} {color} area={grades} {zones} grades={grades ? lengthM : undefined} mark={pointer} {format} bind:width />
    </div>
{/snippet}

{#snippet chip()}{pointer === undefined ? '' : formatDistance(pointer * lengthM)}{/snippet}

<Window {title} wide {onClose}>
    {#snippet children(close)}
        {#if part}<p class="part">{partText}</p>{/if}
        <div class="body" class:beside={times?.zones}>
            <div><Lanes label={title} {rows} {ticks} {pointer} {chip} {part} onpointer={x => pointer = x ?? undefined} /></div>
            {#if times?.zones}
                <section>
                    <h3>Time in each zone</h3>
                    <ZoneTimes channel={times} {unit} {missing} list />
                </section>
            {/if}
        </div>
        <dl>
            {#each numbers as number (number.label)}<div><dt>{number.label}</dt><dd>{number.value}</dd></div>{/each}
        </dl>
        <!-- The link opens the Settings, which are a window too: this window closes first. -->
        {#if times && !times.zones && missing}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="limit" onclick={event => { if ((event.target as Element).closest('a')) close(); }}><ZoneTimes channel={times} {unit} {missing} /></div>
        {/if}
    {/snippet}
</Window>

<style>
    .part {
        margin: -4px 0 12px;
        color: var(--ink-soft);
        font-size: 13px;
        font-variant-numeric: tabular-nums;
    }
    .body {
        display: grid;
        grid-template-columns: minmax(0, 1fr);
        gap: 20px 28px;
    }
    .beside {
        grid-template-columns: minmax(0, 1fr) 200px;
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
    dl {
        display: flex;
        gap: 40px;
        margin: 18px 0 0;
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
