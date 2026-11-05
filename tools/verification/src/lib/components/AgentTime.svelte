<script lang="ts">
  import { onMount } from 'svelte';
  import type { AgentTime, AgentTimeBucket, AgentTool, AgentTotals } from '$lib/types';
  import { api, compact, hours, message } from './api';
  import StackedColumns, { type Column, type Series } from './StackedColumns.svelte';

  /** Bottom to top. The waiting buckets sit on the baseline, so their growth reads directly. */
  const series: (Series & { id: AgentTimeBucket })[] = [
    { id: 'build', label: 'Builds and tests', color: '#eb6834' },
    { id: 'ci', label: 'Waiting on CI', color: '#1baf7a' },
    { id: 'tools', label: 'Other tools', color: '#eda100' },
    { id: 'model', label: 'Thinking and writing', color: '#2a78d6' }
  ];
  const contextSeries: Series[] = [{ id: 'context', label: 'Context read per session', color: '#6250d6' }];
  const ranges = [{ days: 30, label: '30 days' }, { days: 90, label: '90 days' }, { days: 365, label: '1 year' }];
  /** Tool output is counted in characters; about four make one token. */
  const CHARS_PER_TOKEN = 4;
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
  /** Every period of the range, empty ones too, so a quiet week reads as one. */
  $: starts = ((): string[] => {
    if (!data) return [];
    const first = Date.parse(`${data.from}T00:00:00Z`);
    const monday = first - ((new Date(first).getUTCDay() + 6) % 7) * DAY_MS;
    const out: string[] = [];
    for (let at = data.weekly ? monday : first; at <= Date.now(); at += (data.weekly ? 7 : 1) * DAY_MS) out.push(iso(at));
    return out;
  })();
  $: byStart = new Map((data?.periods ?? []).map(p => [p.start, p]));
  $: label = (start: string) => (data?.weekly ? `Week of ${short(start)}` : short(start));
  $: columns = starts.map((start): Column => ({ label: label(start), values: Object.fromEntries(series.map(s => [s.id, byStart.get(start)?.[s.id] ?? 0])) }));
  $: contextColumns = starts.map((start): Column => {
    const p = byStart.get(start);
    return { label: label(start), values: { context: p?.sessions ? context(p) / p.sessions : 0 } };
  });

  const time = (t: AgentTotals) => t.model + t.tools + t.build + t.ci;
  const context = (t: AgentTotals) => t.input + t.cache_read + t.cache_write;
  const ratio = (part: number, whole: number) => (whole ? part / whole : 0);
  const change = (a: number, b: number) => (b ? `${a >= b ? '+' : '−'}${Math.abs(Math.round((100 * (a - b)) / b))}%` : '');
  const points = (a: number, b: number) => `${a >= b ? '+' : '−'}${Math.abs(100 * (a - b)).toFixed(1)} points`;
  const against = (text: string) => (text ? `${text} against the 7 days before` : 'No data for the 7 days before');
  const percent = (value: number) => `${Math.round(100 * value)}%`;
  $: now = data?.recent.now;
  $: before = data?.recent.before;

  /** One row per key over the range. `line` is the time per call in each period. */
  function group(tools: AgentTool[], keep: (t: AgentTool) => boolean) {
    const index = new Map(starts.map((s, i) => [s, i]));
    const groups = new Map<string, { kind: AgentTool['kind']; key: string; calls: number; seconds: number; failed: number; chars: number; sessions: number; repeated: number; per: { calls: number; seconds: number }[] }>();
    for (const t of tools.filter(keep)) {
      const id = `${t.kind}\n${t.key}`;
      const g = groups.get(id) ?? { kind: t.kind, key: t.key, calls: 0, seconds: 0, failed: 0, chars: 0, sessions: 0, repeated: 0, per: starts.map(() => ({ calls: 0, seconds: 0 })) };
      for (const f of ['calls', 'seconds', 'failed', 'chars', 'sessions', 'repeated'] as const) g[f] += t[f];
      const slot = g.per[index.get(t.start) ?? -1];
      if (slot) { slot.calls += t.calls; slot.seconds += t.seconds; }
      groups.set(id, g);
    }
    return [...groups.values()].map(g => {
      const half = Math.floor(g.per.length / 2);
      const mean = (list: typeof g.per) => { const n = list.reduce((s, p) => s + p.calls, 0); return n ? list.reduce((s, p) => s + p.seconds, 0) / n : 0; };
      return { ...g, early: mean(g.per.slice(0, half)), late: mean(g.per.slice(half)), line: g.per.map(p => (p.calls ? p.seconds / p.calls : null)) };
    });
  }
  $: slowest = data ? group(data.tools, t => t.kind !== 'tool').sort((a, b) => b.seconds - a.seconds).slice(0, 15) : [];
  $: outputs = data ? group(data.tools, () => true).sort((a, b) => b.chars - a.chars).slice(0, 15) : [];
  $: allChars = data?.tools.reduce((s, t) => s + t.chars, 0) ?? 0;
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
  const tokens = (chars: number) => compact(chars / CHARS_PER_TOKEN);
</script>
<div class="page-heading row">
  <div><div class="eyebrow">Development</div><h1>Agent time</h1><p class="muted">Where agent sessions spend their time and context, from the transcript each session reports. Time a person spends before a prompt is not counted.</p></div>
  <div class="segmented" role="group" aria-label="Range">{#each ranges as r}<button aria-pressed={range === r.days} disabled={loading} on:click={() => { range = r.days; load(); }}>{r.label}</button>{/each}</div>
</div>
{#if error}<div class="alert error" role="alert">{error} <button on:click={load}>Try again</button></div>{/if}
{#if loading && !data}<p class="muted" role="status">Loading agent time…</p>
{:else if data && !data.periods.length}<div class="empty"><p class="muted">No agent session has reported in this range.</p></div>
{:else if data && now && before}
  <h2 class="part">Time</h2>
  <div class="tiles">
    <section class="tile"><div class="eyebrow">Agent time, last 7 days</div><div class="n">{hours(time(now))} <small>{now.sessions} sessions</small></div><div class="small muted">{against(change(time(now), time(before)))}</div></section>
    {#each series.slice(0, 2) as s}
      <section class="tile"><div class="eyebrow"><i style:background={s.color}></i>{s.label}, last 7 days</div><div class="n">{percent(ratio(now[s.id], time(now)))} <small>{hours(now[s.id])}</small></div><div class="small muted">{against(time(before) ? points(ratio(now[s.id], time(now)), ratio(before[s.id], time(before))) : '')}</div></section>
    {/each}
  </div>
  <div class="legend" aria-label="Legend">{#each [...series].reverse() as s}<span><i style:background={s.color}></i>{s.label}</span>{/each}</div>
  <section class="chart"><h2>Share of agent time</h2><p class="small muted">Each column is 100 % of the agent time in that {data.weekly ? 'week' : 'day'}.</p><StackedColumns {columns} {series} share label="Share of agent time per bucket, per period" /></section>
  <section class="chart"><h2>Agent hours</h2><p class="small muted">All sessions together. More sessions make taller columns.</p><StackedColumns {columns} {series} label="Agent hours per bucket, per period" /></section>
  <section class="chart">
    <h2>Slowest build and CI commands</h2>
    <p class="small muted">The commands with the most total time in the range. The line is the mean time per call in each {data.weekly ? 'week' : 'day'}; the change compares the second half of the range with the first. A failed call exited with an error or printed a test failure. Repeated counts the sessions that ran the command 3 or more times in one {data.weekly ? 'week' : 'day'}.</p>
    {#if slowest.length}
      <div class="scroll"><table>
        <thead><tr><th>Command</th><th>Kind</th><th class="num">Calls</th><th class="num">Failed</th><th class="num">Repeated</th><th class="num">Total</th><th class="num">Per call</th><th>Per call over time</th><th class="num">Change</th></tr></thead>
        <tbody>{#each slowest as c}<tr>
          <td><code>{c.key}</code></td><td>{c.kind === 'ci' ? 'CI' : 'Build'}</td><td class="num">{c.calls}</td>
          <td class="num" class:warning={c.failed / c.calls > 0.2}>{percent(c.failed / c.calls)}</td>
          <td class="num">{c.repeated} of {c.sessions}</td>
          <td class="num">{hours(c.seconds)}</td><td class="num">{seconds(c.seconds / c.calls)}</td>
          <td><svg class="spark" viewBox="0 0 120 28" aria-hidden="true"><path d={spark(c.line)} stroke={c.kind === 'ci' ? series[1].color : series[0].color} /></svg></td>
          <td class="num" class:warning={c.early && c.late > c.early * 1.2}>{c.early && c.late ? change(c.late, c.early) : '—'}</td>
        </tr>{/each}</tbody>
      </table></div>
    {:else}<p class="muted">No build or CI command in this range.</p>{/if}
  </section>

  <h2 class="part">Context</h2>
  <div class="tiles">
    <section class="tile"><div class="eyebrow">Context read per session, last 7 days</div><div class="n">{compact(ratio(context(now), now.sessions))} <small>tokens</small></div><div class="small muted">{against(change(ratio(context(now), now.sessions), ratio(context(before), before.sessions)))}</div></section>
    <section class="tile"><div class="eyebrow">Read from cache, last 7 days</div><div class="n">{percent(ratio(now.cache_read, context(now)))}</div><div class="small muted">{against(context(before) ? points(ratio(now.cache_read, context(now)), ratio(before.cache_read, context(before))) : '')}</div></section>
    <section class="tile"><div class="eyebrow">Tool output per session, last 7 days</div><div class="n">{tokens(ratio(now.chars, now.sessions))} <small>tokens</small></div><div class="small muted">{against(change(ratio(now.chars, now.sessions), ratio(before.chars, before.sessions)))}</div></section>
  </div>
  <section class="chart"><h2>Context read per session</h2><p class="small muted">The input tokens the model reads in all turns of a session, cached or not. Large tool output and long sessions make it grow, because every later turn reads them again.</p><StackedColumns columns={contextColumns} series={contextSeries} format={compact} unit={1} label="Input tokens per session, per period" /></section>
  <section class="chart">
    <h2>Commands and tools with the most output</h2>
    <p class="small muted">Output goes into the context and is read again in every later turn. Tokens are estimated as one per {CHARS_PER_TOKEN} characters.</p>
    {#if outputs.length}
      <div class="scroll"><table>
        <thead><tr><th>Command or tool</th><th class="num">Calls</th><th class="num">Output</th><th class="num">Per call</th><th class="num">Share of all output</th></tr></thead>
        <tbody>{#each outputs as o}<tr><td><code>{o.key}</code></td><td class="num">{o.calls}</td><td class="num">{tokens(o.chars)}</td><td class="num">{tokens(o.chars / o.calls)}</td><td class="num">{percent(ratio(o.chars, allChars))}</td></tr>{/each}</tbody>
      </table></div>
    {:else}<p class="muted">No tool output in this range.</p>{/if}
  </section>
  <details class="chart"><summary>Show the numbers as a table</summary>
    <div class="scroll"><table>
      <thead><tr><th>{data.weekly ? 'Week of' : 'Day'}</th><th class="num">Sessions</th>{#each series as s}<th class="num">{s.label}</th>{/each}<th class="num">Context read</th><th class="num">Output tokens</th></tr></thead>
      <tbody>{#each starts as start}{@const p = byStart.get(start)}<tr><td>{short(start)}</td><td class="num">{p?.sessions ?? 0}</td>{#each series as s}<td class="num">{hours(p?.[s.id] ?? 0)}</td>{/each}<td class="num">{compact(p ? context(p) : 0)}</td><td class="num">{compact(p?.output ?? 0)}</td></tr>{/each}</tbody>
    </table></div>
  </details>
{/if}
<style>
  .segmented { display: flex; gap: 4px; }
  .segmented button[aria-pressed=true] { background: var(--soft); font-weight: 650; }
  .part { margin: 8px 0 12px; }
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
