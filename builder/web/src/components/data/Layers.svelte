<script lang="ts">
    import { span } from "../../lib/data/shapes";

    // The progress of each layer that a run bakes. A tiled layer names its leaves `<i>-<j>`. With
    // `now` and the start of a layer, its time left is its time so far for each step done.
    let {
        layers,
        running,
        now = null,
    }: {
        layers: { layer: string; done: number; total: number; leaves_baking?: string[]; started?: string | null }[];
        running: boolean;
        now?: number | null;
    } = $props();

    function progress({ done, total, leaves_baking = [], started = null }: (typeof layers)[number]): string {
        if (done === total) return "done";
        const left = now && started && done ? ` · about ${span(((now - Date.parse(started)) * (total - done)) / done)} left` : "";
        if (running) return done || leaves_baking.length ? `${done} of ${total}${left}` : "waits";
        return done ? `stopped at ${done} of ${total}` : "not started";
    }
</script>

<section class="card" aria-labelledby="layers">
    <h2 id="layers">Layers in this run</h2>
    {#each layers as layer (layer.layer)}
        <div class="layer">
            <b>{layer.layer}</b>
            <span class={layer.done === layer.total ? "ready" : "soft"}>{progress(layer)}</span>
            <progress class:done={layer.done === layer.total} max={layer.total} value={layer.done} aria-label={layer.layer}></progress>
            {#if layer.leaves_baking?.length}
                <span class="leaves">
                    baking {layer.leaves_baking.slice(0, 12).join(" · ")}{layer.leaves_baking.length > 12
                        ? ` and ${layer.leaves_baking.length - 12} more`
                        : ""}
                </span>
            {/if}
        </div>
    {/each}
</section>

<style>
    .layer {
        display: flex;
        flex-wrap: wrap;
        justify-content: space-between;
        gap: 2px 12px;
        padding: 4px 0;
    }
    progress {
        flex-basis: 100%;
        height: 8px;
        accent-color: var(--running);
    }
    progress.done {
        accent-color: var(--ready);
    }
    .leaves {
        flex-basis: 100%;
        font-family: var(--mono);
        font-size: 13px;
        color: var(--running);
    }
</style>
