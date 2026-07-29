<script lang="ts">
    // A preset card's picture: that preset's own demo map, rendered live at the panel's own
    // 240×320 by the firmware render path compiled to wasm (B2 #899).
    //
    // Three things this component is careful about:
    //
    //  * **Nothing loads until it is seen.** The wasm module is ~60 kB gzipped and a demo map is
    //    70–800 kB; a visitor who never scrolls to the styles pays for neither. An
    //    IntersectionObserver starts the work, and every import here is dynamic so the bundler
    //    keeps it all out of the entry chunk (`lib/preview/bundle.test.ts`).
    //  * **Every card frames the same ground.** The camera is aimed at the one bbox the bake
    //    pinned, never at a map's own header bbox — those differ per preset, and a preset that
    //    got a slightly wider view would win the comparison for the wrong reason.
    //  * **The renderer owns real memory** (≈370 kB of reader cache and render scratch per open
    //    map, plus the map bytes). It is released on unmount; wasm-bindgen has no GC hook.
    //
    // Only the selected card is `interactive`. Drag-to-pan on every card in a grid would fight
    // page scrolling on a phone for no benefit — one live map is the affordance, the rest are
    // pictures that happen to be rendered rather than photographed.

    import { onMount } from "svelte";
    import type { Preview } from "../lib/preview/bridge";

    let {
        presetId,
        label,
        interactive = false,
        fallback,
    }: {
        /** Which preset's demo map to render. */
        presetId: string;
        /** Preset name, for the canvas's accessible label. */
        label: string;
        /** Whether pointer gestures pan and zoom this card. */
        interactive?: boolean;
        /**
         * A published still to fall back to (the catalog's optional `preset.preview`, OBCC §2).
         * The live render wins whenever there is a demo map: it is the same renderer at the same
         * resolution, and it does not go stale when the preset is restyled. This covers the case
         * the bake cannot — a preset whose demo map has not been baked yet.
         */
        fallback?: string;
    } = $props();

    /** `absent` is not a failure: a preset added since the last bake run has no demo map yet. */
    type State = "waiting" | "loading" | "ready" | "absent" | "failed";

    let phase = $state<State>("waiting");
    let canvas = $state<HTMLCanvasElement>();
    let host = $state<HTMLElement>();
    let preview: Preview | null = null;
    /** False once the card is gone; the async load checks it before keeping a wasm object. */
    let mounted = false;

    /** The blit target, allocated once. Reused so a drag does not churn a 300 kB `ImageData`. */
    let image: ImageData | null = null;

    /** Blit the current frame, if it changed. The renderer decides; this never repaints blindly. */
    function paint(force = false) {
        const ctx = canvas?.getContext("2d");
        if (!preview || !ctx) return;
        if (!force && !preview.is_dirty()) return;
        image ??= ctx.createImageData(preview.width, preview.height);
        // `frame()` is a view over wasm memory that any later call may detach, so it is copied
        // into the persistent buffer immediately rather than held.
        image.data.set(preview.frame());
        ctx.putImageData(image, 0, 0);
    }

    async function start() {
        if (phase !== "waiting") return;
        phase = "loading";
        try {
            const [{ openPreview }, { demoMapFor, previewIndex }] = await Promise.all([
                import("../lib/preview/bridge"),
                import("../lib/preview/demoMaps"),
            ]);
            const [index, map] = await Promise.all([previewIndex(), demoMapFor(presetId)]);
            if (!map) {
                phase = "absent";
                return;
            }
            const p = await openPreview(map);
            // Racing an unmount: `destroy` ran while the fetch was in flight, so this instance is
            // already orphaned and must not leak its wasm memory.
            if (!mounted) {
                p.free();
                return;
            }
            const b = index.bbox;
            p.fit_bbox(b.min_lon, b.min_lat, b.max_lon, b.max_lat);
            preview = p;
            phase = "ready";
            // The canvas only exists once `phase` is "ready", so wait a tick for the DOM.
            await Promise.resolve();
            paint(true);
        } catch (e) {
            console.error(`preset preview (${presetId}):`, e);
            phase = "failed";
        }
    }

    onMount(() => {
        mounted = true;
        // No IntersectionObserver (jsdom, very old browsers): render immediately rather than
        // never. The lazy path is an optimisation, not a correctness requirement.
        let io: IntersectionObserver | null = null;
        if (typeof IntersectionObserver === "undefined" || !host) {
            void start();
        } else {
            io = new IntersectionObserver(
                (entries) => {
                    if (entries.some((e) => e.isIntersecting)) {
                        io?.disconnect();
                        void start();
                    }
                },
                // A little early, so the picture is there by the time it is looked at.
                { rootMargin: "200px" },
            );
            io.observe(host);
        }
        return () => {
            mounted = false;
            io?.disconnect();
            preview?.free();
            preview = null;
        };
    });

    // --- interaction (selected card only) ---------------------------------

    let dragging = $state(false);
    let last: { x: number; y: number } | null = null;

    /** Screen pixels to frame pixels: the canvas is drawn at 240×320 but laid out smaller. */
    function scale(): number {
        const rect = canvas?.getBoundingClientRect();
        return rect && rect.width > 0 && preview ? preview.width / rect.width : 1;
    }

    function down(e: PointerEvent) {
        if (!interactive || !preview) return;
        dragging = true;
        last = { x: e.clientX, y: e.clientY };
        (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }

    function move(e: PointerEvent) {
        if (!dragging || !last || !preview) return;
        const k = scale();
        preview.pan((e.clientX - last.x) * k, (e.clientY - last.y) * k);
        last = { x: e.clientX, y: e.clientY };
        paint();
    }

    function up(e: PointerEvent) {
        if (!dragging) return;
        dragging = false;
        last = null;
        (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    }

    function wheel(e: WheelEvent) {
        if (!interactive || !preview) return;
        e.preventDefault();
        preview.zoom_by(e.deltaY < 0 ? 1.15 : 1 / 1.15);
        paint();
    }

    /** Keyboard equivalent of the drag, so the live card is not pointer-only. */
    function key(e: KeyboardEvent) {
        if (!interactive || !preview) return;
        const step = 40;
        const moves: Record<string, [number, number]> = {
            ArrowLeft: [step, 0],
            ArrowRight: [-step, 0],
            ArrowUp: [0, step],
            ArrowDown: [0, -step],
        };
        if (e.key in moves) {
            e.preventDefault();
            preview.pan(...moves[e.key]);
        } else if (e.key === "+" || e.key === "=") {
            preview.zoom_by(1.3);
        } else if (e.key === "-") {
            preview.zoom_by(1 / 1.3);
        } else if (e.key === "0" || e.key === "Escape") {
            preview.reset();
        } else {
            return;
        }
        paint();
    }
</script>

<div class="preview" class:live={interactive} bind:this={host}>
    {#if phase === "ready"}
        <canvas
            bind:this={canvas}
            width="240"
            height="320"
            class:grab={interactive}
            class:grabbing={dragging}
            tabindex={interactive ? 0 : -1}
            aria-label={interactive
                ? `${label} preset, rendered on a demo map. Drag or use the arrow keys to explore; +/- to zoom, 0 to reset.`
                : `${label} preset, rendered on a demo map.`}
            onpointerdown={down}
            onpointermove={move}
            onpointerup={up}
            onpointercancel={up}
            onwheel={wheel}
            onkeydown={key}
            ondblclick={() => {
                preview?.reset();
                paint();
            }}
        ></canvas>
    {:else if fallback && (phase === "absent" || phase === "failed")}
        <img src={fallback} alt={`${label} preset, rendered on a demo map.`} />
    {:else if phase === "failed"}
        <p class="note small">Preview unavailable</p>
    {:else if phase === "absent"}
        <p class="note small">No preview baked yet</p>
    {:else}
        <div class="skeleton" aria-hidden="true"></div>
    {/if}
</div>

<style>
    .preview {
        /* The panel's own shape, so the card reserves its space before the map arrives and
           nothing reflows when it does. */
        aspect-ratio: 240 / 320;
        width: 100%;
        border-radius: 8px;
        overflow: hidden;
        border: 1px solid var(--line);
        background: var(--parchment-2);
        display: grid;
        place-items: center;
    }

    canvas,
    img {
        width: 100%;
        height: 100%;
        display: block;
        object-fit: cover;
        /* Downscaled on most cards, so let the browser filter; nearest-neighbour at a fractional
           ratio would alias the 1 px hairlines the panel draws roads with. */
        touch-action: pan-y;
    }

    canvas.grab {
        cursor: grab;
        /* The live card owns both axes while a drag is in flight, or the page scrolls out from
           under the map on a phone. Only this card — see the component header. */
        touch-action: none;
    }

    canvas.grabbing {
        cursor: grabbing;
    }

    canvas:focus-visible {
        outline: 2px solid var(--forest);
        outline-offset: -2px;
    }

    .note {
        color: var(--ink-faint);
        text-align: center;
        padding: 0 8px;
    }

    .skeleton {
        width: 100%;
        height: 100%;
        background: linear-gradient(105deg, var(--parchment-2) 40%, var(--parchment) 50%, var(--parchment-2) 60%);
        background-size: 300% 100%;
        animation: sweep 1.4s ease-in-out infinite;
    }

    @keyframes sweep {
        from {
            background-position: 100% 0;
        }
        to {
            background-position: 0 0;
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .skeleton {
            animation: none;
        }
    }
</style>
