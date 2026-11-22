<!-- Path difficulty as one range of T grades: the upper end is the hardest grade allowed, the lower end, once moved, asks
     for a part of at least that grade. A grade shows its drawing and description after a second under the pointer, at once
     on a key focus, and stays open after a click or a tap. -->
<script lang="ts">
    import { on } from 'svelte/events';
    import Icon from '../../lib/ui/Icon.svelte';
    import { GRADE_TOP, type Query } from './search';
    import { GRADES } from './words';

    let { query, ungraded, upper = false, onchange }: {
        query: Query;
        /** Only the hardest grade: no lower end and no routes with an unknown grade, as for long trails. */
        upper?: boolean;
        /** The rows with no grade that "Include routes with an unknown grade" keeps. */
        ungraded: number;
        onchange: (change: Partial<Query>) => void;
    } = $props();

    const HOVER_MS = 1000;
    const [low, high] = $derived(query.grade);
    const limited = $derived(low > 0 || high < GRADE_TOP);

    /** The grade whose drawing shows, and the grade that a click keeps open. */
    let shown = $state<number>(), kept = $state<number>(), group = $state<HTMLElement>();
    let timer: ReturnType<typeof setTimeout> | undefined;
    function hover(grade: number) {
        clearTimeout(timer);
        if (shown !== undefined) shown = grade;
        else timer = setTimeout(() => shown = grade, HOVER_MS);
    }
    function leave() {
        clearTimeout(timer);
        shown = kept;
    }
    function click(grade: number) {
        clearTimeout(timer);
        shown = kept = kept === grade ? undefined : grade;
    }
    // A click outside or Escape closes a kept drawing.
    $effect(() => {
        if (kept === undefined) return;
        const close = () => { shown = kept = undefined; };
        const stops = [on(document, 'click', event => { if (!group?.contains(event.target as Node)) close(); }),
            on(document, 'keydown', event => { if (event.key === 'Escape') close(); })];
        return () => { for (const stop of stops) stop(); };
    });
    $effect(() => () => clearTimeout(timer));

    const ANGLES = [7, 14, 24, 34, 44, 56], WIDTHS = [9, 6, 3.5, 2.5], ROCK_COUNT = [0, 0, 3, 5, 7, 8];
    const ROCKS = [[40, 7], [92, 5], [196, 8], [236, 6], [64, 9], [214, 10], [20, 8], [262, 9]];
</script>

<div class="grade" bind:this={group}>
    <div class="head">
        <button type="button" class="info" aria-expanded={kept !== undefined} onclick={event => { event.stopPropagation(); click(high); }}>
            <Icon name="info" size={16} />What the grades mean
        </button>
    </div>
    <div class="rail" style:--low={low} style:--high={high}>
        {#if !upper}
            <input type="range" min="0" max={GRADE_TOP} step="1" value={low} aria-label="Has a part of at least this grade" aria-valuetext="T{low + 1}"
                oninput={event => onchange({ grade: [Math.min(Number(event.currentTarget.value), high), high] })} />
        {/if}
        <input type="range" min="0" max={GRADE_TOP} step="1" value={high} aria-label="Hardest grade allowed" aria-valuetext="T{high + 1}"
            oninput={event => onchange({ grade: [low, Math.max(Number(event.currentTarget.value), low)] })} />
    </div>
    <div class="labels">
        {#each GRADES as [name], grade (grade)}
            <button type="button" class:in={grade >= low && grade <= high} class:showing={shown === grade} style:left="{grade * 20}%" aria-label="T{grade + 1}, {name}"
                onpointerenter={() => hover(grade)} onpointerleave={leave} onfocus={event => { if (event.currentTarget.matches(':focus-visible')) shown = grade; }} onblur={leave}
                onclick={() => click(grade)}>T{grade + 1}</button>
        {/each}
    </div>
    {#if shown !== undefined}
        <div class="drawing" role="tooltip">
            {@render sketch(shown)}
            <p><b>T{shown + 1} · {GRADES[shown][0]}</b>{GRADES[shown][1]}</p>
        </div>
    {/if}
    {#if limited && !upper}
        <label class="unknown"><input type="checkbox" checked={query.ungraded} onchange={event => onchange({ ungraded: event.currentTarget.checked })} />
            Include routes with an unknown grade<span class="count">{ungraded}</span></label>
    {/if}
</div>

<!-- A hill at the steepness of the grade, its path, its rocks and the chain of T3. -->
{#snippet sketch(grade: number)}
    {@const t = Math.tan(ANGLES[grade] * Math.PI / 180)}
    {@const y = (x: number) => 62 - (x - 140) * t}
    <svg viewBox="0 0 280 112" aria-hidden="true">
        <rect class="sky" width="280" height="112" />
        <path class="hill" d="M-10 {y(-10)}L290 {y(290)}V130H-10Z" />
        {#if grade < 4}
            <path class="path" d="M-10 {y(-10)}L290 {y(290)}" stroke-width={WIDTHS[grade]} stroke-dasharray={grade > 1 ? grade === 2 ? '9 6' : '2 8' : undefined} />
        {/if}
        {#each ROCKS.slice(0, ROCK_COUNT[grade]) as [x, r], i (i)}
            <path class="rock" d="M{x - r} {y(x - r) + 1}L{x - r * 0.2} {y(x) - r * (1 + grade * 0.25)}L{x + r} {y(x + r) + 1}Z" />
        {/each}
        {#if grade === 2}
            <path class="chain" d="M150 {y(150) - 13}Q175 {y(175) - 6} 200 {y(200) - 13}Q225 {y(225) - 6} 250 {y(250) - 13}M150 {y(150)}v-13M200 {y(200)}v-13M250 {y(250)}v-13" />
        {/if}
        <g class="walker" transform="translate({118 - grade * 1.5} {y(128) - 31}) scale(1.35)">
            <path d="M15 4a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Zm-3 3 4 4h3M13 6l-3 7 5 3-1 6m-4-9-3 5-4 3M11 7l-3-1-2 6 3 1m11-3-2 12" />
        </g>
    </svg>
{/snippet}

<style>
    .grade {
        position: relative;
    }
    .head {
        display: flex;
        justify-content: flex-end;
        margin-bottom: 4px;
    }
    .info {
        display: inline-flex;
        align-items: center;
        gap: 5px;
        min-height: 26px;
        padding: 0 8px 0 5px;
        border: 0;
        border-radius: 6px;
        background: none;
        color: var(--ink-soft);
        font: 12.5px var(--sans);
    }
    .info:hover, .info[aria-expanded="true"] {
        background: var(--parchment-2);
        color: var(--ink);
    }
    /* Two range inputs share one rail; only their thumbs take the pointer. */
    .rail {
        position: relative;
        height: 24px;
        margin: 0 10px;
    }
    .rail::before, .rail::after {
        content: "";
        position: absolute;
        top: 10px;
        height: 4px;
        border-radius: 2px;
    }
    .rail::before {
        right: 0;
        left: 0;
        background: var(--parchment-3);
    }
    .rail::after {
        right: calc((5 - var(--high)) / 5 * 100%);
        left: calc(var(--low) / 5 * 100%);
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
        width: 18px;
        height: 18px;
        border: 2px solid var(--ink);
        border-radius: 50%;
        background: var(--panel);
        pointer-events: auto;
        cursor: pointer;
        -webkit-appearance: none;
    }
    .rail input::-moz-range-thumb {
        width: 14px;
        height: 14px;
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
    .labels {
        position: relative;
        height: 26px;
        margin: -6px 10px 0;
    }
    .labels button {
        position: absolute;
        top: 0;
        min-width: 30px;
        height: 26px;
        padding: 0 4px;
        border: 0;
        border-radius: 5px;
        background: none;
        color: var(--ink-soft);
        font: 400 12.5px var(--sans);
        transform: translateX(-50%);
    }
    .labels .in {
        color: var(--ink);
        font-weight: 600;
    }
    .labels button:hover, .labels .showing {
        background: var(--parchment-2);
    }
    .drawing {
        position: absolute;
        z-index: 6;
        top: calc(100% - 4px);
        right: 0;
        left: 0;
        display: flex;
        flex-direction: column;
        gap: 8px;
        padding: 8px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--panel);
        box-shadow: var(--planner-shadow);
        pointer-events: none;
    }
    .drawing svg {
        display: block;
        width: 100%;
        height: auto;
        border-radius: 6px;
    }
    .sky {
        fill: var(--parchment-2);
    }
    .hill {
        fill: var(--parchment-3);
    }
    .path {
        fill: none;
        stroke: var(--panel);
        stroke-linecap: round;
    }
    .rock {
        fill: var(--ink-soft);
        opacity: .55;
    }
    .walker {
        fill: none;
        stroke: var(--ink);
        stroke-width: 1.7;
        stroke-linecap: round;
        stroke-linejoin: round;
    }
    .chain {
        fill: none;
        stroke: var(--ink-soft);
        stroke-width: 1.2;
    }
    .drawing p {
        margin: 0 2px;
        font-size: 13px;
    }
    .drawing b {
        display: block;
    }
    .unknown {
        display: flex;
        align-items: center;
        gap: 9px;
        min-height: 30px;
        font-size: 13px;
    }
    .unknown input {
        margin: 0;
        accent-color: var(--ink);
    }
    .count {
        margin-left: auto;
        color: var(--ink-soft);
        font-variant-numeric: tabular-nums;
    }
</style>
