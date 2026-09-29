<script lang="ts">
    import { trailImage } from '../../lib/planner/trail-markers';
    let { symbol, description }: { symbol: string; description: string } = $props();
    let canvas: HTMLCanvasElement | undefined = $state();
    $effect(() => { if (canvas) { const image = trailImage(symbol); if (image) canvas.getContext('2d')?.putImageData(image, 0, 0); } });
</script>
<span role="img" aria-label={description || `Trail marker: ${symbol}`} title={description || symbol}><canvas bind:this={canvas} width="48" height="48" aria-hidden="true"></canvas></span>
<style>span { display: flex; flex: 0 0 24px; } canvas { width: 24px; height: 24px; }</style>
