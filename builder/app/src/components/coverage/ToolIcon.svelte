<script lang="ts">
    // The selection-tool marks, drawn rather than typeset (the old rail used
    // ◧ ▭ ◠ glyphs nobody could read). One component so the rail and the parts
    // list agree about what a lasso looks like. Stroke-drawn in currentColor,
    // matching the field-guide line work; the picked set is the approved
    // "field marks" wireframe (2026-08-09).

    let { kind, size = 20 }: { kind: "region" | "box" | "corridor" | "lasso"; size?: number } = $props();
</script>

<svg
    viewBox="0 0 20 20"
    width={size}
    height={size}
    fill="none"
    stroke="currentColor"
    stroke-width="1.7"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
>
    {#if kind === "region"}
        <!-- a territory with an inner boundary: a mapped subdivision -->
        <path d="M3.5 8 L6.5 3.5 L12 3 L16.5 6.5 L16 12.5 L11 16.5 L5 14.5 Z" />
        <path d="M6.5 3.5 L9.5 9.5 L5 14.5" stroke-width="1.2" stroke-dasharray="2 2" opacity="0.75" />
    {:else if kind === "box"}
        <!-- the marching-ants marquee -->
        <rect x="4" y="5" width="12" height="10" rx="1" stroke-dasharray="3 2.6" />
    {:else if kind === "corridor"}
        <!-- a route and its buffer -->
        <path d="M4.5 16.5 C8 13, 8 9.5, 11 7.5 S15 4.5, 16 3.5" stroke-width="1.8" />
        <path
            d="M2.5 14 C6 10.5, 6 7.5, 9 5.5 S12.5 2.8, 13.5 2"
            stroke-width="1.1"
            stroke-dasharray="2.4 2.2"
            opacity="0.7"
        />
        <path
            d="M6.8 18.5 C10 15.5, 10 12, 13 10 S17.5 6.5, 18.3 5.6"
            stroke-width="1.1"
            stroke-dasharray="2.4 2.2"
            opacity="0.7"
        />
    {:else}
        <!-- the loop and its tail -->
        <path
            d="M10.5 3.2 C5.5 3.2 3.2 6 4.6 8.8 C6 11.6 12.5 12.6 15.4 10.2 C18 8 16.4 3.9 12.4 3.4 C11.8 3.3 11.1 3.2 10.5 3.2 Z"
        />
        <path d="M5.2 9.6 C6.6 12.2 4.6 13.4 3.4 16.6" stroke-width="1.5" />
        <circle cx="5" cy="9.3" r="1.1" fill="currentColor" stroke="none" />
    {/if}
</svg>
