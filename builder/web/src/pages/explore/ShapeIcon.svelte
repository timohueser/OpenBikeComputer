<!-- The picture of a shape. A lift shape draws the walked half solid and the ride dotted, so it says which half the lift
     takes. -->
<script lang="ts">
    import type { Shape } from './search';

    let { shape, size = 18 }: { shape: Shape; size?: number } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    {#if shape === 'loop'}
        <path d="M5 19c-3-6 1-14 8-14s9 8 4 12-9 2-12 2Z" /><circle cx="5" cy="19" r="1.7" fill="currentColor" />
    {:else if shape === 'back'}
        <path d="M5 19c3-3 3-8 7-9s5-3 7-5" /><circle cx="5" cy="19" r="1.7" fill="currentColor" /><circle cx="19" cy="5" r="1.7" />
    {:else}
        <path d="M4 20c4-2 6-8 9-10s4-4 7-6" /><path d="M4 20 20 4" stroke-dasharray="1.5 2.5" />
        <path d={shape === 'liftUp' ? 'M15 4h5v5' : 'M4 15v5h5'} />
    {/if}
</svg>
