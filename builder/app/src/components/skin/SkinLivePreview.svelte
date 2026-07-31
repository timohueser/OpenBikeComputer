<script lang="ts">
    import { onMount } from "svelte";
    import type { SkinEntry } from "../../lib/catalog/manifest";
    import { openLiveSkinPreview, type LiveSkinPreview } from "../../lib/skin/preview";

    let { schemaJson, skin }: { schemaJson: string; skin: SkinEntry } = $props();

    let canvas = $state<HTMLCanvasElement>();
    let preview = $state<LiveSkinPreview | null>(null);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let renderRaf = 0;

    function paint() {
        if (!canvas || !preview) return;
        const pixels = new Uint8ClampedArray(preview.frame());
        const context = canvas.getContext("2d");
        if (!context) {
            error = "This browser cannot draw the live preview.";
            return;
        }
        context.putImageData(new ImageData(pixels, preview.width, preview.height), 0, 0);
    }

    function queuePaint(nextSkin: string) {
        if (!preview) return;
        cancelAnimationFrame(renderRaf);
        renderRaf = requestAnimationFrame(() => {
            try {
                preview?.setSkin(nextSkin);
                paint();
                error = null;
            } catch (cause) {
                error = cause instanceof Error ? cause.message : String(cause);
            }
        });
    }

    onMount(() => {
        let live = true;
        void openLiveSkinPreview(schemaJson, JSON.stringify(skin))
            .then((opened) => {
                if (!live) {
                    opened.free();
                    return;
                }
                preview = opened;
                loading = false;
                paint();
            })
            .catch((cause) => {
                if (!live) return;
                loading = false;
                error = cause instanceof Error ? cause.message : String(cause);
            });
        return () => {
            live = false;
            cancelAnimationFrame(renderRaf);
            preview?.free();
            preview = null;
        };
    });

    $effect(() => {
        // JSON.stringify intentionally reads every nested style field, so a
        // swatch or number edit invalidates exactly one animation-frame render.
        queuePaint(JSON.stringify(skin));
    });
</script>

<div class="preview" aria-busy={loading}>
    <canvas bind:this={canvas} width="240" height="240" aria-label="Live device rendering of Teningen"></canvas>
    {#if loading}
        <div class="state small">Opening Teningen…</div>
    {:else if error}
        <div class="state error small" role="alert">{error}</div>
    {/if}
</div>
<p class="caption small faint">Teningen · 5 m/px · device colors</p>

<style>
    .preview {
        position: relative;
        width: min(100%, 360px);
        aspect-ratio: 1;
        overflow: hidden;
        background: var(--parchment-2);
        border: 1px solid var(--parchment-3);
        border-radius: 13px;
    }

    canvas {
        display: block;
        width: 100%;
        height: 100%;
        image-rendering: pixelated;
    }

    .state {
        position: absolute;
        inset: 0;
        display: grid;
        place-items: center;
        padding: 22px;
        text-align: center;
        color: var(--ink-faint);
        background: rgba(243, 240, 223, 0.9);
    }

    .state.error {
        color: var(--coral);
    }

    .caption {
        margin: 7px 0 0;
        text-align: center;
    }
</style>
