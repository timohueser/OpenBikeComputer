<script lang="ts">
    import type { SkinPreviewFrame } from "../../lib/skin/preview";

    let { frame, label }: { frame: SkinPreviewFrame; label: string } = $props();
    let canvas = $state<HTMLCanvasElement>();

    $effect(() => {
        if (!canvas) return;
        const context = canvas.getContext("2d");
        if (!context) return;
        context.putImageData(new ImageData(Uint8ClampedArray.from(frame.pixels), frame.width, frame.height), 0, 0);
    });
</script>

<span role="img" aria-label={label}>
    <canvas bind:this={canvas} width={frame.width} height={frame.height} aria-hidden="true"></canvas>
</span>

<style>
    span,
    canvas {
        display: block;
        width: 100%;
        aspect-ratio: 1;
    }

    canvas {
        image-rendering: pixelated;
    }
</style>
