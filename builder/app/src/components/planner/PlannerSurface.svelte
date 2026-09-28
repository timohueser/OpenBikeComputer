<script lang="ts">
    import { surfaceRuns, surfaceWindow } from '../../lib/planner/surface-data';
    import type { RoutingLine, Surface } from '../../lib/planner/routing';
    let { line, from, to, onHover }: { line?: RoutingLine; from: number; to: number; onHover: (progress: number | null) => void } = $props();
    const colors: Record<Surface, string> = { Paved: '#536674', Compacted: '#7d8b63', Gravel: '#ba9c5c', Dirt: '#a36d49', Rough: '#875f72', Unknown: '#b8b5ac' };
    const data = $derived(surfaceRuns(line));
    const visible = $derived(surfaceWindow(data.runs, from, to));
    let position = $state<number | null>(null);
    const at = $derived(Math.max(from, Math.min(to, position ?? from)));
    const current = $derived(data.runs.find(run => run.to > at) ?? data.runs.at(-1));
    const description = $derived(current ? `${current.surface === 'Unknown' ? 'Unknown surface' : current.surface} · ${Math.round((data.shares.get(current.surface) ?? 0) * 100) || '<1'}% of route` : 'Surface unavailable');
    function inspect(value: number) { position = Math.max(from, Math.min(to, value)); onHover(position); }
    function leave(event: PointerEvent) { if (document.activeElement !== event.currentTarget) { position = null; onHover(null); } }
    function key(event: KeyboardEvent) {
        const index = visible.findIndex(run => run.to > at);
        let target;
        if (event.key === 'ArrowRight') target = visible[Math.min(visible.length - 1, index + 1)];
        else if (event.key === 'ArrowLeft') target = visible[Math.max(0, index - 1)];
        else if (event.key === 'Home') target = visible[0];
        else if (event.key === 'End') target = visible.at(-1);
        else return;
        event.preventDefault();
        if (target) inspect((target.from + target.to) / 2);
    }
</script>

<div class="surface">
    <div class="bar" role="slider" tabindex="0" aria-label="Surface along route" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(at * 100)} aria-valuetext={description}
        onpointermove={(event) => { const box = event.currentTarget.getBoundingClientRect(); inspect(from + (event.clientX - box.left) / box.width * (to - from)); }}
        onclick={(event) => { const box = event.currentTarget.getBoundingClientRect(); inspect(from + (event.clientX - box.left) / box.width * (to - from)); }}
        onpointerleave={leave} onfocus={() => inspect(from)} onblur={() => { position = null; onHover(null); }} onkeydown={key}>
        {#each visible as run, i (i)}
            <span style:left={`${(run.from - from) / (to - from) * 100}%`} style:width={`${(run.to - run.from) / (to - from) * 100}%`} style:background={colors[run.surface]}></span>
        {/each}
        {#if position !== null && current}<i style:left={`${(at - from) / (to - from) * 100}%`}></i>{/if}
    </div>
    <div class="label">{position === null ? 'Surface · hover or focus to inspect' : description}</div>
</div>

<style>
    .surface { margin: 2px 12px 0 48px; }
    .bar { position: relative; height: 24px; cursor: crosshair; border-radius: 2px; }
    .bar span { position: absolute; top: 8px; height: 8px; }
    .bar i { position: absolute; top: 3px; height: 18px; width: 2px; background: var(--ink); border: 1px solid var(--panel); }
    .bar:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .label { font: 11px var(--sans); color: var(--ink-soft); font-variant-numeric: tabular-nums; }
</style>
