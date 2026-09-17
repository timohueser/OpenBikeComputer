<script lang="ts">
  import { onMount } from 'svelte';
  import type { Catalog, Revision } from '$lib/types';
  import { coverageProgress } from '$lib/coverage';
  import { api, message } from './api';
  export let revision: Revision;
  export let catalog: Catalog;
  let revisions: Revision[] = [];
  let error = '';
  onMount(async () => { try { revisions = await api<Revision[]>('/api/revisions'); } catch (e) { error = message(e); } });
  $: now = coverageProgress(revision.requirements, catalog);
  $: history = [...revisions].sort((a, b) => a.id - b.id).map(r => {
    const p = coverageProgress(r.requirements, catalog);
    return { id: r.id, at: new Date(r.createdAt).getTime(), covered: p.states.covered, assessed: p.states.covered + p.states.partial, active: p.active };
  });
  const pct = (part: number, whole: number) => whole ? `${(100 * part / whole).toFixed(1)}%` : '0%';
  const day = (at: number) => new Date(at).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });

  /** Step chart geometry: time on x, count on y, the ceiling is the largest active count. */
  const W = 900, H = 220, L = 40, R = 16, T = 12, B = 28;
  $: top = Math.max(1, ...history.map(h => h.active));
  $: first = history[0]?.at ?? 0;
  $: span = Math.max(1, (history[history.length - 1]?.at ?? 0) - first);
  $: x = (at: number) => history.length < 2 ? (L + W - R) / 2 : L + (W - L - R) * (at - first) / span;
  $: y = (n: number) => T + (H - T - B) * (1 - n / top);
  $: step = (key: 'covered' | 'assessed') => history.map((h, i) => i ? `H${x(h.at).toFixed(1)} V${y(h[key]).toFixed(1)}` : `M${x(h.at).toFixed(1)} ${y(h[key]).toFixed(1)}`).join(' ');
  $: ticks = [0, Math.round(top / 2), top];
</script>
<div class="page-heading"><div class="eyebrow">Product verification</div><h1>Coverage</h1><p class="muted">How much of what we promise is proven, and how that changes over time.</p></div>
{#if error}<div class="alert error" role="alert">{error}</div>{/if}
<div class="tiles">
  <section class="tile" aria-label="Requirements">
    <div class="eyebrow">Requirements</div><div class="n">{now.states.covered} <small>of {now.active} covered</small></div>
    <div class="bar"><span class="covered" style:width={pct(now.states.covered, now.active)}></span><span class="partial" style:width={pct(now.states.partial, now.active)}></span><span class="review" style:width={pct(now.states['needs-review'], now.active)}></span></div>
    <div class="legend"><span><i class="covered"></i>{now.states.covered} covered</span><span><i class="partial"></i>{now.states.partial} partial</span><span><i class="review"></i>{now.states['needs-review']} needs review</span><span><i class="none"></i>{now.states.unassessed} not assessed</span></div>
  </section>
  <section class="tile" aria-label="Acceptance criteria">
    <div class="eyebrow">Acceptance criteria</div><div class="n">{now.criteria.covered} <small>of {now.criteria.total} covered</small></div>
    <div class="bar"><span class="covered" style:width={pct(now.criteria.covered, now.criteria.total)}></span></div>
    <div class="legend"><span><i class="covered"></i>{now.criteria.covered} with evidence, no gap</span><span><i class="none"></i>{now.criteria.total - now.criteria.covered} with a gap or no evidence</span></div>
  </section>
  <section class="tile" aria-label="Tests">
    <div class="eyebrow">Tests</div><div class="n">{now.tests.cited} <small>of {now.tests.catalog} cited</small></div>
    <div class="bar"><span class="covered" style:width={pct(now.tests.cited, now.tests.catalog)}></span></div>
    <div class="legend"><span><i class="covered"></i>{now.tests.cited} cited by a plan</span><span><i class="none"></i>{now.tests.catalog - now.tests.cited} not cited</span><span><i class="manual"></i>{now.tests.manual} manual</span></div>
  </section>
</div>
<section class="chart" aria-label="Covered requirements per revision">
  <div class="row"><h3>Covered requirements per revision</h3>{#if history.length}<span class="small muted">r{history[0].id} – r{history[history.length - 1].id} · {day(history[0].at)} – {day(history[history.length - 1].at)}</span>{/if}</div>
  <svg viewBox="0 0 {W} {H}" role="img" aria-label="Covered and assessed requirements over time">
    {#each ticks as t}<line x1={L} x2={W - R} y1={y(t)} y2={y(t)} class="grid" /><text x={L - 6} y={y(t) + 4} class="axis" text-anchor="end">{t}</text>{/each}
    {#if history.length}
      <path d={step('assessed')} class="assessed" /><path d={step('covered')} class="covered" />
      {#if history.length === 1}<circle cx={x(first)} cy={y(history[0].covered)} r="4" class="dot" />{/if}
      <text x={L} y={H - 6} class="axis">{day(first)}</text>{#if history.length > 1}<text x={W - R} y={H - 6} class="axis" text-anchor="end">{day(history[history.length - 1].at)}</text>{/if}
    {/if}
  </svg>
  <div class="legend"><span><i class="covered"></i>Covered</span><span><i class="partial"></i>Assessed (covered + partial)</span></div>
</section>
<style>
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(230px, 1fr)); gap: 14px; margin: 0 0 18px; }
  .tile, .chart { background: var(--surface); border: 1px solid var(--line); border-radius: 9px; padding: 16px 18px; }
  .n { font-size: 28px; letter-spacing: -1px; font-weight: 600; line-height: 1.1; margin: 4px 0 2px; }
  .n small { font-size: 14px; letter-spacing: 0; color: var(--muted); font-weight: 500; }
  .bar { display: flex; height: 12px; border-radius: 999px; overflow: hidden; margin: 10px 0 8px; background: var(--soft); }
  .bar span { display: block; }
  .legend { display: flex; gap: 16px; flex-wrap: wrap; font-size: 12px; color: var(--muted); }
  .legend i { display: inline-block; width: 9px; height: 9px; border-radius: 50%; margin-right: 5px; vertical-align: -1px; }
  .covered { background: var(--forest); } .partial { background: var(--amber); } .review { background: #a1452f; } .none { background: #c9ccbd; } .manual { background: #7c8db5; }
  .chart h3 { margin-bottom: 4px; }
  svg { display: block; width: 100%; height: auto; margin-top: 6px; }
  .grid { stroke: var(--line); }
  .axis { font-size: 11px; fill: var(--muted); }
  path { fill: none; stroke-width: 2.5; stroke-linejoin: round; }
  path.covered { stroke: var(--forest); } path.assessed { stroke: var(--amber); stroke-width: 2; stroke-dasharray: 4 3; }
  .dot { fill: var(--forest); }
</style>
