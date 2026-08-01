<script lang="ts">
    import { onMount } from "svelte";
    import type { SkinEntry } from "../../lib/catalog/manifest";
    import { openLiveSkinPreview, type LivePreviewStats, type LiveSkinPreview } from "../../lib/skin/preview";
    import { keyboardCameraAction, PreviewDragSession, wheelZoomFactor } from "../../lib/skin/previewInteraction";

    let { schemaJson, skin }: { schemaJson: string; skin: SkinEntry } = $props();

    let canvas = $state<HTMLCanvasElement>();
    let preview = $state<LiveSkinPreview | null>(null);
    let loading = $state(true);
    let error = $state<string | null>(null);
    let stats = $state<LivePreviewStats | null>(null);
    let dragging = $state(false);
    let renderRaf = 0;
    const drag = new PreviewDragSession();

    function paint() {
        if (!canvas || !preview) return;
        const pixels = new Uint8ClampedArray(preview.frame());
        const context = canvas.getContext("2d");
        if (!context) {
            error = "This browser cannot draw the live preview.";
            return;
        }
        context.putImageData(new ImageData(pixels, preview.width, preview.height), 0, 0);
        stats = preview.stats();
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

    function logicalPoint(event: { clientX: number; clientY: number }): { x: number; y: number } {
        if (!canvas || !preview) return { x: 0, y: 0 };
        const rect = canvas.getBoundingClientRect();
        return {
            x: ((event.clientX - rect.left) * preview.width) / Math.max(rect.width, 1),
            y: ((event.clientY - rect.top) * preview.height) / Math.max(rect.height, 1),
        };
    }

    function renderCamera(action: () => void) {
        if (!preview) return;
        action();
        paint();
        error = null;
    }

    function pointerDown(event: PointerEvent) {
        if (!preview || !event.isPrimary || (event.pointerType === "mouse" && event.button !== 0)) return;
        const point = logicalPoint(event);
        if (!drag.begin(event.pointerId, point.x, point.y)) return;
        event.currentTarget instanceof HTMLElement && event.currentTarget.focus();
        canvas?.setPointerCapture(event.pointerId);
        dragging = true;
        event.preventDefault();
    }

    function pointerMove(event: PointerEvent) {
        const point = logicalPoint(event);
        const delta = drag.move(event.pointerId, point.x, point.y);
        if (!delta) return;
        event.preventDefault();
        renderCamera(() => preview?.panBy(delta.dx, delta.dy));
    }

    function endPointer(event: PointerEvent) {
        if (!drag.end(event.pointerId)) return;
        if (canvas?.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
        dragging = false;
    }

    function cancelPointer() {
        drag.cancel();
        dragging = false;
    }

    function wheel(event: WheelEvent) {
        if (!preview) return;
        event.preventDefault();
        const point = logicalPoint(event);
        renderCamera(() => preview?.zoomAt(wheelZoomFactor(event.deltaY, event.deltaMode), point.x, point.y));
    }

    function zoom(factor: number) {
        renderCamera(() => preview?.zoomAt(factor, (preview?.width ?? 0) / 2, (preview?.height ?? 0) / 2));
    }

    function resetCamera() {
        renderCamera(() => preview?.resetCamera());
    }

    function keydown(event: KeyboardEvent) {
        const action = keyboardCameraAction(event.key);
        if (!action || !preview) return;
        event.preventDefault();
        if (action.kind === "pan") renderCamera(() => preview?.panBy(action.dx, action.dy));
        else if (action.kind === "zoom") zoom(action.factor);
        else resetCamera();
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
            cancelPointer();
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

<div class="preview" class:dragging aria-busy={loading}>
    <canvas
        bind:this={canvas}
        width="240"
        height="240"
        tabindex="0"
        aria-label="Interactive live device rendering of Teningen. Drag to pan; use the mouse wheel, plus and minus keys to zoom; arrow keys pan; Home resets."
        aria-describedby="skin-preview-status"
        onpointerdown={pointerDown}
        onpointermove={pointerMove}
        onpointerup={endPointer}
        onpointercancel={endPointer}
        onlostpointercapture={cancelPointer}
        onwheel={wheel}
        onkeydown={keydown}
    ></canvas>
    {#if loading}
        <div class="state small">Opening Teningen…</div>
    {:else if error}
        <div class="state error small" role="alert">{error}</div>
    {/if}
</div>
<div class="controls" aria-label="Preview camera controls">
    <button type="button" class="camera-button" aria-label="Zoom preview out" onclick={() => zoom(0.8)}>−</button>
    <p id="skin-preview-status" class="caption small faint" aria-live="polite">
        Teningen
        {#if stats}
            · {stats.metersPerPixel < 10 ? stats.metersPerPixel.toFixed(1) : stats.metersPerPixel.toFixed(0)} m/px
            · LOD {stats.lodIndex}/{Math.max(stats.lodCount - 1, 0)}
        {/if}
        · device colors
    </p>
    <button type="button" class="camera-button" aria-label="Zoom preview in" onclick={() => zoom(1.25)}>+</button>
    <button type="button" class="reset-button small" onclick={resetCamera}>Reset view</button>
</div>

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
        cursor: grab;
        touch-action: none;
        outline-offset: -3px;
    }

    .dragging canvas {
        cursor: grabbing;
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

    .controls {
        display: grid;
        grid-template-columns: 30px minmax(0, 1fr) 30px;
        align-items: center;
        gap: 5px;
        margin-top: 7px;
    }

    .caption {
        margin: 0;
        text-align: center;
    }

    .camera-button,
    .reset-button {
        min-height: 30px;
        padding: 2px 7px;
        color: var(--ink);
        background: var(--parchment-2);
        border: 1px solid var(--line);
        border-radius: 8px;
    }

    .reset-button {
        grid-column: 1 / -1;
        justify-self: center;
    }
</style>
