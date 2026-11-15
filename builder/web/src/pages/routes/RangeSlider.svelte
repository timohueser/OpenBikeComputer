<!-- A range of a chip: bars with the count of the items at each step, and two handles under them. -->
<script lang="ts">
    import { rangeText, scale, STEPS, type Range } from './filter';

    let { values, range, onchange, label, format }: {
        /** The value of each item, in the units of the rider. */
        values: number[];
        range: Range | undefined;
        /** `undefined` when both handles are at the ends. */
        onchange: (range: Range | undefined) => void;
        label: string;
        format: (value: number) => string;
    } = $props();

    const step = $derived(scale(values, range).step);
    const top = $derived(STEPS * step);
    // A stored end that is off the step shows at the nearest step, and keeps its value until the rider moves its handle.
    const low = $derived(range ? Math.max(0, Math.min(Math.round(range[0] / step), STEPS - 1)) : 0);
    const high = $derived(range && range[1] !== Infinity ? Math.max(Math.min(Math.round(range[1] / step), STEPS), low + 1) : STEPS);
    /** The last bar also counts each value from `top` up. */
    const counts = $derived.by(() => {
        const counts = Array<number>(STEPS).fill(0);
        for (const value of values) counts[Math.max(0, Math.min(Math.floor(value / step), STEPS - 1))]++;
        return counts;
    });
    const most = $derived(Math.max(1, ...counts));

    function move(end: 0 | 1, input: HTMLInputElement) {
        const wanted = Math.round(Number(input.value) / step);
        // A handle stops one step before the other one.
        const at = end === 0 ? Math.min(wanted, high - 1) : Math.max(wanted, low + 1);
        input.value = String(at * step);
        if (at === (end === 0 ? low : high)) return;
        const min = end === 0 ? at * step : range?.[0] ?? 0;
        const max = end === 1 ? (at === STEPS ? Infinity : at * step) : range?.[1] ?? Infinity;
        onchange(min <= 0 && max === Infinity ? undefined : [min, max]);
    }
</script>

<div class="slider">
    <p class="reading">{range ? rangeText(range, format) : 'Any'}</p>
    <div class="bars" aria-hidden="true">
        {#each counts as count, i (i)}<span class:on={i >= low && i < high} style:height="{count / most * 100}%"></span>{/each}
    </div>
    <div class="rail" style:--low={low} style:--high={high} style:--steps={STEPS}>
        <input type="range" min="0" max={top} {step} value={low * step} aria-label="{label}, from" aria-valuetext={low ? format(low * step) : 'Any'}
            oninput={event => move(0, event.currentTarget)} />
        <input type="range" min="0" max={top} {step} value={high * step} aria-label="{label}, to" aria-valuetext={high < STEPS ? format(high * step) : 'Any'}
            oninput={event => move(1, event.currentTarget)} />
    </div>
</div>

<style>
    /* The side padding is half a thumb, so that a handle at step k stands under the edge between bar k - 1 and bar k. */
    .slider {
        width: 256px;
        padding: 4px 10px 2px;
    }
    .reading {
        margin: 0 0 8px;
        color: var(--ink);
        font-size: 13px;
        font-weight: 600;
    }
    .bars {
        display: flex;
        align-items: flex-end;
        gap: 2px;
        height: 44px;
    }
    .bars span {
        flex: 1;
        border-radius: 2px 2px 0 0;
        background: var(--parchment-3);
    }
    .bars .on {
        background: var(--ink-soft);
    }
    /* Two range inputs share one rail; only their thumbs take the pointer. */
    .rail {
        position: relative;
        height: 24px;
    }
    .rail::before, .rail::after {
        content: "";
        position: absolute;
        top: 10px;
        height: 4px;
        border-radius: 2px;
    }
    .rail::before {
        left: 0;
        right: 0;
        background: var(--parchment-2);
    }
    .rail::after {
        left: calc(var(--low) / var(--steps) * 100%);
        right: calc((var(--steps) - var(--high)) / var(--steps) * 100%);
        background: var(--ink-soft);
    }
    .rail input {
        position: absolute;
        z-index: 1;
        inset: 0 -10px;
        width: calc(100% + 20px);
        height: 24px;
        margin: 0;
        background: none;
        pointer-events: none;
        appearance: none;
        -webkit-appearance: none;
    }
    .rail input::-webkit-slider-thumb {
        width: 20px;
        height: 20px;
        border: 2px solid var(--ink);
        border-radius: 50%;
        background: var(--panel);
        pointer-events: auto;
        cursor: pointer;
        -webkit-appearance: none;
    }
    .rail input::-moz-range-thumb {
        width: 16px;
        height: 16px;
        border: 2px solid var(--ink);
        border-radius: 50%;
        background: var(--panel);
        pointer-events: auto;
        cursor: pointer;
    }
    .rail input:focus-visible {
        outline: none;
    }
    .rail input:focus-visible::-webkit-slider-thumb {
        outline: 2px solid var(--forest);
        outline-offset: 2px;
    }
</style>
