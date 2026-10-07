<script lang="ts">
    import { surfaceRuns, surfaceWindow } from '../../lib/planner/surface-data';
    import type { RoutingLine, Surface } from '../../lib/planner/routing';
    let { line, from, to, walking = false, onHover }: { walking?: boolean; line?: RoutingLine; from: number; to: number; onHover: (progress: number | null) => void } = $props();
    const colors: Record<Surface, string> = { Paved: '#536674', Compacted: '#7d8b63', Gravel: '#ba9c5c', Dirt: '#a36d49', Rough: '#875f72', Unknown: '#b8b5ac' };
    const helpId = $props.id();
    const data = $derived(surfaceRuns(line));
    const visible = $derived(surfaceWindow(data.runs, from, to));
    const hasPushing = $derived(!walking && data.runs.some(run => run.pushing));
    const pushingDistance = $derived((line?.pushingKm ?? 0) < 1 ? `${Math.round((line?.pushingKm ?? 0) * 1000)} m` : `${line!.pushingKm.toFixed(1)} km`);
    let position = $state<number | null>(null);
    const at = $derived(Math.max(from, Math.min(to, position ?? from)));
    const current = $derived(visible.find(run => run.to > at) ?? visible.at(-1));
    const access = $derived(current?.pushing == null ? 'Access unverified' : walking ? 'Walking' : current.pushing ? 'Push bike' : 'Riding');
    const description = $derived(current ? `${current.surface === 'Unknown' ? 'Unknown surface' : current.surface} · ${Math.round((data.shares.get(current.surface) ?? 0) * 100) || '<1'}% of route · ${access}` : 'Surface unavailable');
    function inspect(value: number) { if (!visible.length) return; position = Math.max(from, Math.min(to, value)); onHover(position); }
    function leave(event: PointerEvent) { if (document.activeElement !== event.currentTarget) { position = null; onHover(null); } }
    function key(event: KeyboardEvent) {
        const found = visible.findIndex(run => run.to > at);
        const index = found < 0 ? visible.length - 1 : found;
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
    <div class="label">
        <strong>Surface</strong>
        {#if position !== null && current}
            <span class="detail"><span class="swatch" style:background={colors[current.surface]}></span>{current.surface}<b>{Math.round((data.shares.get(current.surface) ?? 0) * 100) || '<1'}% <span>of route</span></b><span class="access">{access}</span></span>
        {:else}
            <span class="hint">{hasPushing ? `${pushingDistance} pushing · ` : ''}{visible.length ? 'Hover to inspect' : 'Surface unavailable'}</span>
        {/if}
    </div>
    <span class="sr-only" id={helpId}>Use the arrow keys to inspect adjacent surface and access sections. Percentages refer to the whole route. {#if !walking}The Push strip marks sections where you must push your bike.{/if}</span>
    <div class="bar" class:with-pushing={hasPushing} role="slider" tabindex={visible.length ? 0 : -1} aria-label={walking ? 'Surface along route' : 'Surface and pushing along route'} aria-describedby={helpId} aria-disabled={!visible.length} aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(at * 100)} aria-valuetext={description}
        onpointermove={(event) => { const box = event.currentTarget.getBoundingClientRect(); inspect(from + (event.clientX - box.left) / box.width * (to - from)); }}
        onclick={(event) => { const box = event.currentTarget.getBoundingClientRect(); inspect(from + (event.clientX - box.left) / box.width * (to - from)); }}
        onpointerleave={leave} onfocus={() => inspect(from)} onblur={() => { position = null; onHover(null); }} onkeydown={key}>
        <div class="track">
            {#each visible as run, i (i)}
                <span style:left={`${(run.from - from) / (to - from) * 100}%`} style:width={`${(run.to - run.from) / (to - from) * 100}%`} style:background={colors[run.surface]}></span>
            {/each}
        </div>
        {#if hasPushing}
            <span class="push-label" aria-hidden="true">Push</span>
            <div class="push-track" aria-hidden="true">
                {#each visible.filter(run => run.pushing) as run, i (i)}
                    <span style:left={`${(run.from - from) / (to - from) * 100}%`} style:width={`${(run.to - run.from) / (to - from) * 100}%`}></span>
                {/each}
            </div>
        {/if}
        {#if position !== null && current}<i style:left={`${(at - from) / (to - from) * 100}%`}></i>{/if}
    </div>
</div>

<style>
    .surface { margin: 12px 12px 0 48px; }
    .label { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; min-height: 20px; color: var(--ink); font: 13px var(--sans); }
    strong { font-weight: 600; }
    .detail { display: flex; align-items: center; gap: 7px; min-width: 0; }
    .access { font-size: 12px; font-weight: 600; }
    .swatch { width: 10px; height: 10px; border-radius: 2px; flex: none; }
    b { margin-left: 5px; font-weight: 600; font-variant-numeric: tabular-nums; white-space: nowrap; }
    b span, .hint { font-size: 12px; font-weight: 400; color: var(--ink-soft); }
    .bar { position: relative; height: 28px; cursor: crosshair; border-radius: 3px; }
    .bar.with-pushing { height: 44px; }
    .track { position: absolute; top: 7px; left: 0; right: 0; height: 14px; overflow: hidden; border-radius: 3px; background: var(--parchment-2); }
    .track span { position: absolute; height: 100%; }
    .push-label { position: absolute; top: 27px; right: calc(100% + 10px); color: var(--ink-soft); font-size: 11px; }
    .push-track { position: absolute; top: 31px; left: 0; right: 0; height: 5px; background: var(--parchment-2); }
    .push-track span { position: absolute; height: 100%; background: var(--ink-soft); }
    .bar i { position: absolute; top: 2px; bottom: 2px; width: 3px; transform: translateX(-50%); background: var(--ink); border: 1px solid var(--panel); border-radius: 2px; pointer-events: none; }
    .bar:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
    .bar[aria-disabled="true"] { cursor: default; }
    .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
</style>
