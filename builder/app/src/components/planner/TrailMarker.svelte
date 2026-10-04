<script lang="ts">
    import { trailImage, trailMarker } from '../../lib/planner/trail-markers';
    /** Without a symbol that draws, the `ref` shows as a plain badge, as on French GR and PR routes. */
    let { symbol, description = '', ref }: { symbol?: string; description?: string; ref?: string } = $props();
    const drawn = $derived(!!symbol && !!trailMarker(symbol));
    let canvas: HTMLCanvasElement | undefined = $state();
    $effect(() => { if (canvas && symbol) { const image = trailImage(symbol); if (image) canvas.getContext('2d')?.putImageData(image, 0, 0); } });
</script>
{#if drawn}
    <span role="img" aria-label={description || `Trail marker: ${symbol}`} title={description || symbol}><canvas bind:this={canvas} width="48" height="48" aria-hidden="true"></canvas></span>
{:else if ref}
    <span class="ref" role="img" aria-label={`Route number ${ref}`} title={ref}>{ref}</span>
{/if}
<style>
    span { display: flex; flex: 0 0 24px; }
    canvas { width: 24px; height: 24px; }
    .ref { display: grid; place-items: center; flex: none; min-width: 24px; height: 24px; padding: 0 3px; border: 1.5px solid var(--ink-soft); border-radius: 3px; color: var(--ink-soft); font: 700 8.5px var(--mono); letter-spacing: -.02em; white-space: nowrap; }
</style>
