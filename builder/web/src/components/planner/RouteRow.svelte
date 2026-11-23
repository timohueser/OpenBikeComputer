<!-- A row of a route line: a place on the line in the colour `--route`, with the sign of its pin on the map, its name and
     its place along the route. The rows of a list join into one line. -->
<script lang="ts" module>
    import type { RoutePoint } from '../../lib/planner/editor';

    /** The sign of a stop between the ends of a line. */
    export const stopSign = (point: RoutePoint) => point.turnaround ? 'turn' : point.kind === 'pass' ? 'pass' : point.kind === 'marker' ? 'marker' : 'visit';
    /** The row that a line with no stop shows between its ends. */
    export const hint = { sign: 'add', label: 'To add a stop, click the map or search' } as const;
</script>

<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { Attachment } from 'svelte/attachments';
    import type { ClassValue } from 'svelte/elements';
    import Icon from '../../lib/ui/Icon.svelte';
    import { still } from '../../lib/ui/search-bar/motion';

    let { sign, icon, label, sub, along = '', lit = false, reveal = false, onclick, onHover, children, class: classes, style }: {
        /** A `night` is the place that the rider set, with its `icon`; a `suggested` night is open. `add` is the hint for
         * a first stop. */
        sign: 'start' | 'finish' | 'visit' | 'turn' | 'pass' | 'marker' | 'night' | 'suggested' | 'add';
        icon?: string;
        label: string;
        sub?: string;
        along?: string;
        lit?: boolean;
        /** The row scrolls into view and lights up for a moment. */
        reveal?: boolean;
        /** A row with no click is text. */
        onclick?: () => void;
        /** The pointer or the focus is on the row. */
        onHover?: (on: boolean) => void;
        /** The controls of the row, at its right. */
        children?: Snippet;
        class?: ClassValue;
        style?: string;
    } = $props();

    const icons: Record<string, { name?: string; path?: string } | undefined> = {
        start: { path: 'M8 5v14l11-7z' }, finish: { name: 'flag' }, visit: { name: 'flag' }, turn: { path: 'M7 20V9a5 5 0 0 1 10 0v6m-3-3 3 3 3-3' },
        suggested: { name: 'moon' },
    };
    const mark = $derived(sign === 'night' ? { name: icon } : icons[sign]);
    const note = $derived(sub ?? (sign === 'turn' ? 'Turns back here' : ''));
    const show: Attachment = node => {
        if (reveal) node.scrollIntoView({ block: 'nearest', behavior: still() ? 'auto' : 'smooth' });
    };
</script>

{#snippet place()}
    <span class={['sign', sign]}>{#if mark}<Icon {...mark} size={sign === 'start' ? 12 : sign === 'night' ? 14 : 13} />{/if}</span><span class="name"><strong>{label}</strong>{#if note}<small>{note}</small>{/if}</span>{#if along}<small class="along">{along}</small>{/if}
{/snippet}

<li class={[classes, { hint: sign === 'add' }]} {style}>
    {#if onclick}
        <button type="button" class={['stop', { highlighted: lit, flash: reveal }]} {onclick} onmouseenter={() => onHover?.(true)} onmouseleave={() => onHover?.(false)}
            onfocus={() => onHover?.(true)} onblur={() => onHover?.(false)} {@attach show}>{@render place()}</button>
    {:else}
        <div class="stop">{@render place()}</div>
    {/if}{@render children?.()}
</li>

<style>
    li {
        position: relative;
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
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
    .stop {
        position: relative;
        display: grid;
        grid-template-columns: 28px minmax(0, 1fr) auto;
        column-gap: 12px;
        align-items: center;
        min-height: 48px;
        padding: 2px 4px 2px 10px;
        border: 0;
        border-radius: 10px;
        background: none;
        color: var(--ink);
        font: inherit;
        text-align: left;
    }
    button { cursor: pointer; }
    button:focus-visible { outline: 2px solid var(--ink); outline-offset: -2px; }
    button:hover, .highlighted { background: var(--parchment-2); }
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
    .night { background: var(--ink); color: var(--panel); box-shadow: 0 0 0 2px var(--panel); }
    .suggested { border: 1.5px dashed var(--ink-soft); background: var(--panel); color: var(--ink-soft); }
    /* The hint is a note on the line, not a control: a small empty mark and soft text. */
    .add { width: 10px; height: 10px; border: 1.5px solid var(--line-strong); background: var(--panel); box-shadow: 0 0 0 2px var(--panel); }
    strong, small { display: block; }
    strong { font: 600 14px/1.3 var(--sans); overflow-wrap: anywhere; }
    .hint strong { font: 400 13px/1.35 var(--sans); color: var(--ink-soft); }
    .hint .stop { min-height: 36px; }
    small {
        font-size: 13px;
        color: var(--ink-soft);
    }
    .name small::first-letter { text-transform: uppercase; }
    .along {
        font-variant-numeric: tabular-nums;
        text-align: right;
        white-space: nowrap;
    }
    .flash { animation: flash 1.2s ease-out; }
    @keyframes flash {
        from { background: var(--parchment-2); }
        to { background: transparent; }
    }
    @media (prefers-reduced-motion: reduce) {
        .flash {
            animation: none;
            background: var(--parchment-2);
        }
    }
</style>
