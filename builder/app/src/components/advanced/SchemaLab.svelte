<script lang="ts">
    import { onMount } from "svelte";
    import { buildConfigForSubmit, type SchemaEnvelope } from "../../lib/config/model";
    import type { WorkingEnvelope } from "../../lib/config/storage.svelte";
    import { platform } from "../../lib/platform";
    import type { SchemaPreviewMap, SchemaPreviewStatus } from "../../lib/platform/types";
    import { representativeMpp } from "../../lib/schema/lods";
    import {
        openSchemaRenderer,
        type SchemaRenderer,
        type SchemaRenderStats,
    } from "../../lib/schema/render";
    import { PreviewController, type PreviewPhase } from "../../lib/schema/previewController";

    let { env, schema }: { env: WorkingEnvelope; schema: SchemaEnvelope | null } = $props();

    const service = platform.schemaPreview;
    let source = $state<SchemaPreviewStatus | null>(null);
    let phase = $state<PreviewPhase<SchemaPreviewMap>>({ kind: "idle" });
    let renderer = $state<SchemaRenderer | null>(null);
    let stats = $state<SchemaRenderStats | null>(null);
    let diagnostics = $state<string[]>([]);
    let packDurationMs = $state(0);
    let metersPerPixel = $state(5);
    let canvas = $state<HTMLCanvasElement>();
    let installGeneration = 0;

    const controller = new PreviewController(
        (config: Record<string, unknown>, signal) => {
            if (!service) throw new Error("The schema lab is unavailable in this host.");
            return service.pack(config, signal);
        },
        (next) => {
            const generation = ++installGeneration;
            phase = next;
            if (next.kind === "ready") void install(next.value, generation);
        },
    );

    async function install(result: SchemaPreviewMap, generation: number) {
        try {
            const opened = await openSchemaRenderer(result.bytes);
            if (generation !== installGeneration) {
                opened.free();
                return;
            }
            const previous = renderer;
            renderer = opened;
            previous?.free();
            packDurationMs = result.packDurationMs;
            diagnostics = result.diagnostics;
            opened.setMetersPerPixel(metersPerPixel);
            paint();
        } catch (cause) {
            phase = { kind: "error", message: cause instanceof Error ? cause.message : String(cause) };
        }
    }

    function paint() {
        if (!canvas || !renderer) return;
        const pixels = new Uint8ClampedArray(renderer.frame());
        const context = canvas.getContext("2d");
        if (!context) {
            phase = { kind: "error", message: "This browser cannot draw the device preview." };
            return;
        }
        context.putImageData(new ImageData(pixels, renderer.width, renderer.height), 0, 0);
        stats = renderer.stats();
        metersPerPixel = stats.metersPerPixel;
    }

    function selectScale(value: number) {
        if (!Number.isFinite(value) || value <= 0) return;
        metersPerPixel = value;
        renderer?.setMetersPerPixel(value);
        paint();
    }

    async function refreshSource() {
        if (!service) return;
        try {
            source = await service.status();
        } catch (cause) {
            source = {
                available: false,
                label: "preview source",
                configured: false,
                detail: cause instanceof Error ? cause.message : String(cause),
                bbox: "",
            };
        }
    }

    onMount(() => {
        void refreshSource();
        return () => {
            controller.dispose();
            renderer?.free();
            renderer = null;
        };
    });

    $effect(() => {
        if (!source?.available || !schema) return;
        // Stringify/parse deliberately reads every nested Svelte proxy field and
        // snapshots exactly one native-pack request. Disabled rows are dropped by
        // the same submit normalization an exported config uses.
        const submitted = buildConfigForSubmit(env.config, env.disabled, schema).config;
        const snapshot = JSON.parse(JSON.stringify(submitted)) as Record<string, unknown>;
        controller.schedule(snapshot);
    });
</script>

<section class="lab card" aria-label="Teningen schema preview">
    <div class="intro">
        <div>
            <p class="eyebrow">Maintainer schema lab</p>
            <h3>Teningen on the real device renderer</h3>
            <p class="small faint">
                Edits debounce into one native pack of a fixed crop. Nothing here bakes a region or changes the
                product builders. Expect roughly 5–15 seconds depending on the machine and cache, not an instant restyle.
            </p>
        </div>
        <span class="state small" class:error={phase.kind === "error"}>
            {#if phase.kind === "waiting"}Waiting for edits…
            {:else if phase.kind === "packing"}Packing Teningen…
            {:else if phase.kind === "ready" && renderer}Packed in {(packDurationMs / 1000).toFixed(1)} s
            {:else if phase.kind === "error"}{phase.message}
            {:else}Checking source…{/if}
        </span>
    </div>

    {#if source && !source.available}
        <div class="missing" role="status">
            <strong>Preview source unavailable</strong>
            <span class="small">{source.detail}</span>
            <code>obc web preview-source</code>
            <button type="button" class="btn ghost" onclick={refreshSource}>Check again</button>
        </div>
    {:else}
        <div class="body">
            <div>
                <div class="screen" aria-busy={phase.kind === "packing" || phase.kind === "waiting"}>
                    <canvas bind:this={canvas} width="240" height="320" aria-label="240 by 320 device map rendering of Teningen"></canvas>
                    {#if !renderer}
                        <div class="screen-state small">{phase.kind === "packing" ? "Packing…" : "Waiting for preview…"}</div>
                    {/if}
                </div>
                <label class="scale small">
                    <span>Scale</span>
                    <input
                        type="number"
                        min="0.5"
                        max="100"
                        step="0.1"
                        value={metersPerPixel}
                        oninput={(event) => selectScale(event.currentTarget.valueAsNumber)}
                    />
                    <span>m/px</span>
                </label>
                <div class="lods" aria-label="Preview each authored LOD">
                    {#each env.config.lods as _lod, index}
                        {@const value = representativeMpp(env.config.lods, index)}
                        <button
                            type="button"
                            class:active={stats?.lodIndex === index}
                            onclick={() => selectScale(value)}
                        >LOD {index}</button>
                    {/each}
                </div>
            </div>

            <div class="metrics">
                <h4>Production frame stats</h4>
                {#if stats}
                    <dl>
                        <dt>Scale / LOD</dt><dd>{stats.metersPerPixel.toFixed(1)} m/px · {stats.lodIndex}/{stats.lodCount - 1}</dd>
                        <dt>Features</dt><dd>{stats.featuresDrawn}/{stats.featuresTried} drawn</dd>
                        <dt>Dropped</dt><dd class:warn={stats.featuresDropped > 0}>{stats.featuresDropped}</dd>
                        <dt>Chunks</dt><dd>{stats.chunksVisited}</dd>
                        <dt>Spans</dt><dd>{stats.spansUsed}/1,152</dd>
                        <dt>Frame points</dt><dd>{stats.pointsDrawn}/4,768</dd>
                        <dt>Frame rings</dt><dd>{stats.ringsUsed}/1,024</dd>
                        <dt>Points tried</dt><dd>{stats.pointsTried}</dd>
                        <dt>Decode overflows</dt><dd class:warn={stats.featureDecodeCapacityDrops > 0}>{stats.featureDecodeCapacityDrops}</dd>
                        <dt>Malformed / map errors</dt><dd class:warn={stats.malformedFeatures + stats.mapErrors > 0}>{stats.malformedFeatures} / {stats.mapErrors}</dd>
                    </dl>
                {:else}
                    <p class="small faint">Stats appear after the first production render.</p>
                {/if}
                <p class="limits small">
                    Per feature: <strong>2,048 points · 32 rings</strong>. Frame: <strong>1,152 spans · 4,768 points · 1,024 rings</strong>.
                </p>
                {#if diagnostics.length}
                    <div class="diagnostics" role="alert">
                        <strong>Pack diagnostics</strong>
                        {#each diagnostics as line}<span class="small">{line}</span>{/each}
                    </div>
                {/if}
            </div>
        </div>
    {/if}
</section>

<style>
    .lab {
        margin-bottom: 14px;
        border-left: 4px solid var(--amber);
    }

    .intro,
    .body,
    .missing {
        display: flex;
        gap: 18px;
    }

    .intro {
        justify-content: space-between;
        align-items: flex-start;
        margin-bottom: 14px;
    }

    h3,
    h4,
    p {
        margin: 0;
    }

    .eyebrow {
        color: var(--olive);
        font-size: 11px;
        font-weight: 800;
        letter-spacing: 0.08em;
        text-transform: uppercase;
    }

    .state {
        max-width: 420px;
        text-align: right;
    }

    .error,
    .warn {
        color: var(--coral);
    }

    .missing {
        align-items: center;
        flex-wrap: wrap;
        padding: 12px;
        background: var(--parchment-2);
        border-radius: 10px;
    }

    .missing .btn {
        margin-left: auto;
    }

    .body {
        align-items: flex-start;
    }

    .screen {
        position: relative;
        width: 240px;
        height: 320px;
        overflow: hidden;
        border: 1px solid var(--line-strong);
        border-radius: 10px;
        background: var(--parchment-2);
    }

    canvas {
        display: block;
        width: 240px;
        height: 320px;
        image-rendering: pixelated;
    }

    .screen-state {
        position: absolute;
        inset: 0;
        display: grid;
        place-items: center;
        color: var(--ink-faint);
    }

    .scale {
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 6px;
        margin-top: 8px;
    }

    .scale input {
        width: 76px;
        padding: 4px 6px;
    }

    .lods {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 4px;
        margin-top: 7px;
    }

    .lods button {
        padding: 4px;
        border: 1px solid var(--line-strong);
        border-radius: 6px;
        background: transparent;
        font-size: 11px;
    }

    .lods button.active {
        color: white;
        background: var(--olive);
    }

    .metrics {
        min-width: 300px;
        flex: 1;
    }

    dl {
        display: grid;
        grid-template-columns: minmax(130px, 1fr) auto;
        gap: 6px 14px;
        margin: 10px 0;
        font-size: 13px;
    }

    dt {
        color: var(--ink-faint);
    }

    dd {
        margin: 0;
        font-variant-numeric: tabular-nums;
    }

    .limits {
        padding-top: 10px;
        border-top: 1px solid var(--line);
    }

    .diagnostics {
        display: grid;
        gap: 4px;
        margin-top: 12px;
        padding: 10px;
        color: var(--coral);
        background: rgba(194, 77, 42, 0.08);
        border-radius: 8px;
    }

    @media (max-width: 760px) {
        .body {
            flex-direction: column;
        }

        .metrics {
            min-width: 0;
            width: 100%;
        }
    }
</style>
