<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_TIME_BUCKETS, type AgentTime, type AgentTimeBucket } from '$lib/types';
  import { api, hours, message } from './api';
  import StackedColumns, { type Column, type Series } from './StackedColumns.svelte';

  /** Bottom to top. The waiting buckets sit on the baseline, so their growth reads directly. */
  const series: (Series & { id: AgentTimeBucket })[] = [
    { id: 'build', label: 'Builds and tests', color: '#eb6834' },
    { id: 'ci', label: 'Waiting on CI', color: '#1baf7a' },
    { id: 'tools', label: 'Other tools', color: '#eda100' },
    { id: 'model', label: 'Thinking and writing', color: '#2a78d6' }
  ];
  const ranges = [{ days: 30, label: '30 days' }, { days: 90, label: '90 days' }, { days: 365, label: '1 year' }];
  const DAY_MS = 86_400_000;
  let range = 90;
  let data: AgentTime | null = null;
  let error = '';
  let loading = true;
  async function load() {
    loading = true; error = '';
    try { data = await api<AgentTime>(`/api/agent-time?days=${range}`); } catch (e) { error = message(e); } finally { loading = false; }
  }
  onMount(load);

  const iso = (at: number) => new Date(at).toISOString().slice(0, 10);
  const short = (day: string) => new Date(`${day}T00:00:00Z`).toLocaleDateString(undefined, { day: 'numeric', month: 'short', timeZone: 'UTC' });
  /** Days for 30 days, weeks from Monday otherwise. Empty periods stay, so a quiet week reads as one. */
  $: weekly = range > 30;
  function periodOf(day: string): string {
    if (!weekly) return day;
    const at = Date.parse(`${day}T00:00:00Z`);
    return iso(at - ((new Date(at).getUTCDay() + 6) % 7) * DAY_MS);
  }
  type Period = { start: string; sessions: number; seconds: Record<AgentTimeBucket, number> };
  $: periods = ((): Period[] => {
    if (!data) return [];
    const map = new Map<string, Period>();
    for (let at = Date.parse(`${data.from}T00:00:00Z`); at <= Date.now(); at += DAY_MS) {
      const start = periodOf(iso(at));
      if (!map.has(start)) map.set(start, { start, sessions: 0, seconds: { model: 0, tools: 0, build: 0, ci: 0 } });
    }
    for (const d of data.days) {
      const p = map.get(periodOf(d.day)); if (!p) continue;
      p.sessions += d.sessions;
      for (const b of AGENT_TIME_BUCKETS) p.seconds[b] += d.seconds[b];
    }
    return [...map.values()];
  })();
  $: columns = periods.map((p): Column => ({ label: weekly ? `Week of ${short(p.start)}` : short(p.start), values: p.seconds }));

  /** The last 7 days against the 7 before. */
  function totals(from: number, to: number) {
    const s = { model: 0, tools: 0, build: 0, ci: 0 } as Record<AgentTimeBucket, number>;
    for (const d of data?.days ?? []) {
      const age = (Date.now() - Date.parse(`${d.day}T00:00:00Z`)) / DAY_MS;
      if (age >= from && age < to) for (const b of AGENT_TIME_BUCKETS) s[b] += d.seconds[b];
    }
    return { ...s, total: AGENT_TIME_BUCKETS.reduce((sum, b) => sum + s[b], 0) };
  }
  $: now = data && totals(0, 7);
  $: before = data && totals(7, 14);
  const share = (w: { total: number } & Record<string, number>, b: string) => (w.total ? w[b] / w.total : 0);
  const change = (a: number, b: number) => (b ? `${a >= b ? '+' : '−'}${Math.abs(Math.round((100 * (a - b)) / b))}%` : '');
  const pp = (a: number, b: number) => `${a >= b ? '+' : '−'}${Math.abs(100 * (a - b)).toFixed(1)} points`;

  /** The build and CI commands with the most time in the range, with the time per call in each period. */
  $: commands = (() => {
    if (!data) return [];
    const index = new Map(periods.map((p, i) => [p.start, i]));
    const groups = new Map<string, { kind: string; key: string; count: number; seconds: number; per: { count: number; seconds: number }[] }>();
    for (const c of data.commands) {
      const id = `${c.kind}\n${c.key}`;
      const g = groups.get(id) ?? { kind: c.kind, key: c.key, count: 0, seconds: 0, per: periods.map(() => ({ count: 0, seconds: 0 })) };
      g.count += c.count; g.seconds += c.seconds;
      const slot = g.per[index.get(periodOf(c.day)) ?? -1];
      if (slot) { slot.count += c.count; slot.seconds += c.seconds; }
      groups.set(id, g);
    }
    return [...groups.values()].sort((a, b) => b.seconds - a.seconds).slice(0, 15).map(g => {
      const half = Math.floor(g.per.length / 2);
      const mean = (list: typeof g.per) => { const n = list.reduce((s, p) => s + p.count, 0); return n ? list.reduce((s, p) => s + p.seconds, 0) / n : 0; };
      return { ...g, mean: g.seconds / g.count, early: mean(g.per.slice(0, half)), late: mean(g.per.slice(half)), line: g.per.map(p => (p.count ? p.seconds / p.count : null)) };
    });
  })();
  function spark(values: (number | null)[]): string {
    const max = Math.max(1, ...values.map(v => v ?? 0));
    const x = (i: number) => 2 + (116 * i) / Math.max(1, values.length - 1);
    let path = '', open = false;
    values.forEach((v, i) => {
      if (v === null) { open = false; return; }
      const point = `${x(i).toFixed(1)} ${(26 - (22 * v) / max).toFixed(1)}`;
      path += open ? `L${point} ` : `M${point} L${point} `; open = true;
    });
    return path;
  }
  const seconds = (s: number) => (s < 90 ? `${Math.round(s)} s` : `${(s / 60).toFixed(1)} min`);
</script>
<div class="page-heading row">
  <div><div class="eyebrow">Development</div><h1>Agent time</h1><p class="muted">Where agent sessions spend their time, from the transcript timestamps each session reports. Time a person spends before a prompt is not counted.</p></div>
  <div class="segmented" role="group" aria-label="Range">{#each ranges as r}<button aria-pressed={range === r.days} disabled={loading} on:click={() => { range = r.days; load(); }}>{r.label}</button>{/each}</div>
</div>
{#if error}<div class="alert error" role="alert">{error} <button on:click={load}>Try again</button></div>{/if}
{#if loading && !data}<p class="muted" role="status">Loading agent time…</p>
{:else if data && !data.days.length}<div class="empty"><p class="muted">No agent session has reported time in this range.</p></div>
{:else if data && now && before}
  <div class="tiles">
    <section class="tile"><div class="eyebrow">Agent time, last 7 days</div><div class="n">{hours(now.total)}</div><div class="small muted">{change(now.total, before.total) || 'No data'} against the 7 days before</div></section>
    {#each series.slice(0, 2) as s}
      <section class="tile"><div class="eyebrow"><i style:background={s.color}></i>{s.label}, last 7 days</div><div class="n">{Math.round(100 * share(now, s.id))}% <small>{hours(now[s.id])}</small></div><div class="small muted">{before.total ? pp(share(now, s.id), share(before, s.id)) : 'No data'} against the 7 days before</div></section>
    {/each}
  </div>
  <div class="legend" aria-label="Legend">{#each [...series].reverse() as s}<span><i style:background={s.color}></i>{s.label}</span>{/each}</div>
  <section class="chart"><h2>Share of agent time</h2><p class="small muted">Each column is 100 % of the agent time in that {weekly ? 'week' : 'day'}.</p><StackedColumns {columns} {series} share label="Share of agent time per bucket, per period" /></section>
  <section class="chart"><h2>Agent hours</h2><p class="small muted">All sessions together. More sessions make taller columns.</p><StackedColumns {columns} {series} label="Agent hours per bucket, per period" /></section>
  <section class="chart">
    <h2>Slowest build and CI commands</h2>
    <p class="small muted">The commands with the most total time in the range. The line is the mean time per call in each {weekly ? 'week' : 'day'}; the change compares the second half of the range with the first.</p>
    {#if commands.length}
      <div class="scroll"><table>
        <thead><tr><th>Command</th><th>Kind</th><th class="num">Calls</th><th class="num">Total</th><th class="num">Per call</th><th>Per call over time</th><th class="num">Change</th></tr></thead>
        <tbody>{#each commands as c}<tr>
          <td><code>{c.key}</code></td><td>{c.kind === 'ci' ? 'CI' : 'Build'}</td><td class="num">{c.count}</td><td class="num">{hours(c.seconds)}</td><td class="num">{seconds(c.mean)}</td>
          <td><svg class="spark" viewBox="0 0 120 28" aria-hidden="true"><path d={spark(c.line)} stroke={c.kind === 'ci' ? series[1].color : series[0].color} /></svg></td>
          <td class="num" class:warning={c.early && c.late > c.early * 1.2}>{c.early && c.late ? change(c.late, c.early) : '—'}</td>
        </tr>{/each}</tbody>
      </table></div>
    {:else}<p class="muted">No build or CI command in this range.</p>{/if}
  </section>
  <details class="chart"><summary>Show the numbers as a table</summary>
    <div class="scroll"><table>
      <thead><tr><th>{weekly ? 'Week of' : 'Day'}</th><th class="num">Session days</th>{#each series as s}<th class="num">{s.label}</th>{/each}</tr></thead>
      <tbody>{#each periods as p}<tr><td>{short(p.start)}</td><td class="num">{p.sessions}</td>{#each series as s}<td class="num">{hours(p.seconds[s.id])}</td>{/each}</tr>{/each}</tbody>
    </table></div>
  </details>
{/if}
<style>
  .segmented { display: flex; gap: 4px; }
  .segmented button[aria-pressed=true] { background: var(--soft); font-weight: 650; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(230px, 1fr)); gap: 14px; margin: 0 0 18px; }
  .tile, .chart { background: var(--surface); border: 1px solid var(--line); border-radius: 9px; padding: 16px 18px; }
  .chart { margin-bottom: 18px; }
  .chart h2 { font-size: 15px; }
  .chart p { margin: 2px 0 10px; }
  .n { font-size: 28px; font-weight: 700; letter-spacing: -.5px; margin: 4px 0; font-variant-numeric: tabular-nums; }
  .n small { font-size: 14px; font-weight: 500; color: var(--muted); }
  .legend { display: flex; gap: 16px; flex-wrap: wrap; font-size: 12px; color: var(--muted); margin: 0 0 10px; }
  i { display: inline-block; width: 9px; height: 9px; border-radius: 2px; margin-right: 5px; vertical-align: -1px; }
  .scroll { overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { text-align: left; padding: 6px 10px 6px 0; border-bottom: 1px solid var(--line); white-space: nowrap; }
  th { font-weight: 650; color: var(--muted); font-size: 12px; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  code { font-family: var(--mono); font-size: 12px; }
  .spark { display: block; width: 120px; height: 28px; }
  .spark path { fill: none; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
  summary { cursor: pointer; font-weight: 650; }
  details[open] summary { margin-bottom: 10px; }
</style>
