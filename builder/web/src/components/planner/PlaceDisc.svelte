<script lang="ts">
    /** The disc of a highlighted place: its icon in white on `--place-disc`, with a count badge for several places. A
     * button that holds it lifts it on hover. With `grow`, it grows from the size of a map dot when it shows. */
    let { icon, count = 1, grow = false }: { icon: string; count?: number; grow?: boolean } = $props();
</script>

<span class="place-disc" class:grow>
    <svg viewBox="0 0 24 24" width="13" height="13" aria-hidden="true"><path d={icon} /></svg>
    {#if count > 1}<b>{count > 99 ? '99+' : count}</b>{/if}
</span>

<style>
    /* The ring takes the colour of the paper of the map, where one is set. */
    .place-disc {
        --ease: cubic-bezier(0.23, 1, 0.32, 1);
        position: relative; display: grid; place-items: center; width: 22px; height: 22px; border-radius: 50%;
        color: #fff; background: var(--place-disc); box-shadow: 0 0 0 1.5px var(--paper, var(--panel)); transition: transform 120ms var(--ease);
    }
    svg { display: block; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
    .place-disc::after { content: ''; position: absolute; inset: 0; border-radius: 50%; box-shadow: 0 3px 8px rgba(28, 27, 20, .35); opacity: 0; transition: opacity 120ms var(--ease); }
    b {
        position: absolute; left: 14px; top: -6px; box-sizing: border-box; min-width: 14px; height: 14px; padding: 0 3px; border-radius: 7px;
        color: var(--ink, #1c1b14); background: var(--panel, #fff); font: 700 9px/14px var(--sans, sans-serif); font-variant-numeric: tabular-nums; text-align: center;
    }
    /* No end frame: the disc grows to its size of the moment, such as the lift of a hover. */
    .grow { animation: grow 120ms var(--ease); }
    @keyframes grow { from { transform: scale(0.35); } }

    @media (hover: hover) and (pointer: fine) {
        :global(button:hover) > .place-disc { transform: scale(1.1); }
        :global(button:hover) > .place-disc::after { opacity: 1; }
    }
    @media (prefers-reduced-motion: reduce) {
        .place-disc { transform: none !important; animation: none; }
    }
</style>
