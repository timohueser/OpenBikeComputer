<script lang="ts" context="module">
  export interface Series { id: string; label: string; color: string }
  export interface Column { label: string; values: Record<string, number> }
</script>
<script lang="ts">
  import { hours } from './api';
  /** Stacked columns, one per period. `format` writes a value, and the axis steps in multiples of `unit`. */
  export let columns: Column[];
  export let series: Series[];
  export let label: string;
  export let format: (value: number) => string = hours;
  export let unit = 3600;
  const W = 900, H = 240, L = 44, R = 12, T = 10, B = 26, GAP = 2, RADIUS = 4;
  let hovered = -1;
  const total = (c: Column) => series.reduce((sum, s) => sum + (c.values[s.id] ?? 0), 0);
  $: top = nice(Math.max(0, ...columns.map(total)) / unit) * unit;
  $: band = (W - L - R) / Math.max(1, columns.length);
  $: width = Math.min(28, band * 0.72);
  $: y = (fraction: number) => T + (H - T - B) * (1 - fraction);
  $: ticks = [0, top / 2, top];
  $: stacks = columns.map((c, i) => {
    let base = 0;
    const parts = series.map(s => {
      const from = base; base += (c.values[s.id] ?? 0) / top;
      return { color: s.color, from, to: base };
    }).filter(p => p.to - p.from > 0);
    return { x: L + band * i + (band - width) / 2, parts };
  });
  function nice(value: number): number {
    if (value <= 0) return 1;
    const step = 10 ** Math.floor(Math.log10(value));
    return [1, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10].map(m => m * step).find(v => v >= value) ?? 10 * step;
  }
  /** A segment with a 2 px gap above it; the top segment gets rounded corners. */
  function segment(x: number, from: number, to: number, last: boolean): string {
    const bottom = y(from), height = Math.max(0, bottom - y(to) - (last ? 0 : GAP));
    const r = last ? Math.min(RADIUS, height, width / 2) : 0, top = bottom - height;
    return `M${x} ${bottom} V${top + r} Q${x} ${top} ${x + r} ${top} H${x + width - r} Q${x + width} ${top} ${x + width} ${top + r} V${bottom} Z`;
  }
  $: labelled = new Set([0, Math.floor((columns.length - 1) / 2), columns.length - 1]);
</script>
<div class="scroll"><div class="stacked">
  <svg viewBox="0 0 {W} {H}" role="img" aria-label={label} on:mouseleave={() => (hovered = -1)}>
    {#each ticks as t}<line x1={L} x2={W - R} y1={y(t / top)} y2={y(t / top)} class="grid" /><text x={L - 6} y={y(t / top) + 4} class="axis" text-anchor="end">{format(t)}</text>{/each}
    {#each stacks as stack, i}
      {#if hovered === i}<rect x={L + band * i} y={T} width={band} height={H - T - B} class="hover" />{/if}
      {#each stack.parts as part, j}<path d={segment(stack.x, part.from, part.to, j === stack.parts.length - 1)} fill={part.color} />{/each}
      {#if labelled.has(i)}<text x={stack.x + width / 2} y={H - 6} class="axis" text-anchor={i === 0 ? 'start' : i === columns.length - 1 ? 'end' : 'middle'}>{columns[i].label}</text>{/if}
      <rect x={L + band * i} y={T} width={band} height={H - T - B} fill="transparent" role="presentation" on:mouseenter={() => (hovered = i)} />
    {/each}
  </svg>
  {#if hovered >= 0}
    {@const c = columns[hovered]}
    <div class="tooltip" style:left="{(100 * (L + band * (hovered + 0.5))) / W}%" class:flip={hovered > columns.length / 2}>
      <strong>{c.label}</strong>
      {#each [...series].reverse() as s}<div><i style:background={s.color}></i>{s.label}<span>{format(c.values[s.id] ?? 0)}{#if series.length > 1 && total(c)}&nbsp;· {Math.round((100 * (c.values[s.id] ?? 0)) / total(c))}%{/if}</span></div>{/each}
      {#if series.length > 1}<div class="sum">Total<span>{format(total(c))}</span></div>{/if}
    </div>
  {/if}
</div></div>
<style>
  /* Below this width the axis text gets too small to read, so the chart scrolls instead. */
  .scroll { overflow-x: auto; }
  .stacked { position: relative; min-width: 600px; }
  svg { display: block; width: 100%; height: auto; }
  .grid { stroke: var(--line); }
  .axis { font-size: 12px; fill: var(--muted); }
  .hover { fill: var(--soft); }
  .tooltip { position: absolute; top: 8px; transform: translateX(12px); min-width: 210px; padding: 9px 11px; background: var(--surface); border: 1px solid var(--line-strong); border-radius: 7px; box-shadow: 0 4px 14px #1c1b141a; font-size: 12px; pointer-events: none; z-index: 2; }
  .tooltip.flip { transform: translateX(calc(-100% - 12px)); }
  .tooltip div { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
  .tooltip span { margin-left: auto; padding-left: 14px; font-variant-numeric: tabular-nums; }
  .tooltip i { width: 9px; height: 9px; border-radius: 2px; flex: none; }
  .tooltip .sum { border-top: 1px solid var(--line); margin-top: 4px; padding-top: 4px; font-weight: 650; }
</style>
