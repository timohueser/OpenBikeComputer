<script lang="ts">
    // The selection-tool marks, drawn rather than typeset (the old rail used
    // ◧ ▭ ◠ glyphs nobody could read). One component so the rail and the parts
    // list agree about what a box looks like. Stroke-drawn in currentColor,
    // matching the field-guide line work; the picked set is the approved
    // "field marks" wireframe.

    let { kind, size = 20 }: { kind: "region" | "box"; size?: number } = $props();
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
    {:else}
        <!-- the marching-ants marquee -->
        <rect x="4" y="5" width="12" height="10" rx="1" stroke-dasharray="3 2.6" />
    {/if}
</svg>
