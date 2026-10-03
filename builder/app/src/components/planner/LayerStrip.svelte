<script lang="ts">
    import LayerInspect from './LayerInspect.svelte';
    import { dateLabel, valueRuns, type DataLayer, type Theme } from '../../lib/planner/layers/data-layer';
    import { profileSamples, sampleIndex } from '../../lib/planner/profile-data';
    import type { RoutingLine } from '../../lib/planner/routing';

    let { layer, line, samples, total, date, theme, from, to, onHover }: {
        layer: DataLayer;
        line: RoutingLine;
        /** The layer data at each coordinate of `line`, or null while it loads. */
        samples: unknown;
        total: number;
        date: string;
        theme: Theme;
        from: number;
        to: number;
        onHover: (progress: number | null) => void;
    } = $props();

    const points = $derived(profileSamples(line));
    const swatches = $derived(layer.swatches(theme));
    const runs = $derived(samples ? valueRuns(layer.classes(samples, date), points.map(point => point.progress)).filter(run => run.to > from && run.from < to) : []);
    let bar: HTMLDivElement;
    let at = $state<{ progress: number; x: number; bottom: number } | null>(null);
    const index = $derived(at ? Math.min(points.length - 1, sampleIndex(points, at.progress)) : -1);
    const inspection = $derived(samples && index >= 0 ? layer.inspect(samples, index, date, theme) : null);
    const height = $derived(index >= 0 ? points[index].height : null);

    function show(progress: number) {
        const box = bar.getBoundingClientRect();
        progress = Math.max(from, Math.min(to, progress));
        at = { progress, x: box.left + (progress - from) / (to - from) * box.width, bottom: innerHeight - box.top + 10 };
        onHover(progress);
    }
    function point(event: PointerEvent) {
        const box = bar.getBoundingClientRect();
        show(from + (event.clientX - box.left) / box.width * (to - from));
    }
    function hide() { at = null; onHover(null); }
    function key(event: KeyboardEvent) {
        const step = { ArrowLeft: -0.02, ArrowRight: 0.02 }[event.key];
        if (!step) return;
        event.preventDefault();
        show((at?.progress ?? from) + step * (to - from));
    }
</script>

<svelte:window onkeydown={(event) => { if (event.key === 'Escape' && at) hide(); }} />

<div class="strip">
    <div class="label">
        <span><strong>{layer.label}</strong><span class="hint">{layer.error || (samples ? `${dateLabel(date)} · hover for past years` : 'Loading…')}</span></span>
        <ul class="legend">
            {#each swatches as swatch (swatch.label)}<li><span class="swatch" class:hatch={swatch.hatch} style:--swatch={swatch.color}></span>{swatch.label}</li>{/each}
        </ul>
    </div>
    <div class="bar" bind:this={bar} role="slider" tabindex={samples ? 0 : -1} aria-label={`${layer.label} along the route on ${dateLabel(date)}`}
        aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round((at?.progress ?? from) * 100)} aria-valuetext={inspection?.headline ?? 'Hover or use the arrow keys for past years'}
        onpointermove={point} onpointerleave={hide} onfocus={() => show(from)} onblur={hide} onkeydown={key}>
        {#each runs as run, i (i)}
            {@const swatch = swatches[run.value]}
            <span class:hatch={swatch.hatch} style:--swatch={swatch.color}
                style:left={`${(Math.max(run.from, from) - from) / (to - from) * 100}%`} style:width={`${(Math.min(run.to, to) - Math.max(run.from, from)) / (to - from) * 100}%`}></span>
        {/each}
        {#if at}<i style:left={`${(at.progress - from) / (to - from) * 100}%`}></i>{/if}
    </div>
</div>
{#if at && inspection}
    <div class="popover" role="tooltip" style:left={`clamp(8px, ${at.x - 184}px, calc(100vw - 376px))`} style:bottom={`${at.bottom}px`}>
        <LayerInspect title={`${layer.label} at km ${(at.progress * total).toFixed(1)}`} detail={height === null ? '' : `${Math.round(height).toLocaleString('en-GB')} m`} {inspection} source={layer.source} />
    </div>
{/if}

<style>
    .strip { margin: 6px 12px 0 48px; }
    .label { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; min-height: 20px; font-size: 13px; }
    strong { font-weight: 600; }
    .hint { margin-left: 8px; font-size: 12px; color: var(--ink-soft); }
    .legend { display: flex; flex-wrap: wrap; justify-content: flex-end; gap: 2px 12px; margin: 0; padding: 0; list-style: none; font-size: 12px; color: var(--ink-soft); }
    .legend li { display: flex; align-items: center; gap: 5px; }
    .swatch { width: 14px; height: 8px; border-radius: 2px; background: var(--swatch); box-shadow: inset 0 0 0 1px var(--line-strong); }
    .hatch { background: repeating-linear-gradient(135deg, transparent 0 2px, var(--swatch) 2px 3.5px); }
    .bar { position: relative; height: 22px; cursor: crosshair; }
    .bar::before { content: ""; position: absolute; inset: 3px 0; border: 1px solid var(--line-strong); border-radius: 3px; }
    .bar > span { position: absolute; top: 4px; height: 14px; background: var(--swatch); }
    .bar i { position: absolute; top: 0; bottom: 0; width: 3px; transform: translateX(-50%); background: var(--ink); border: 1px solid var(--panel); border-radius: 2px; pointer-events: none; }
    .bar:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .popover { position: fixed; z-index: 20; border-radius: 8px; background: var(--panel); box-shadow: var(--planner-shadow); pointer-events: none; }
    @container (max-width: 640px) { .legend { display: none; } }
</style>
