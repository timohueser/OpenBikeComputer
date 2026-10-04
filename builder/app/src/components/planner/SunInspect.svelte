<script lang="ts">
    import { clock, SUN, UNKNOWN, type SunDay } from '../../lib/planner/layers/sun';
    import type { SunClient } from '../../lib/planner/layers/sun-client';
    import type { Coordinate } from '../../lib/planner/map-types';
    let { client, coordinate, date, minute, onTime }: { client: SunClient; coordinate: Coordinate; date: string; minute: number; onTime?: (minute: number) => void } = $props();
    let day = $state.raw<SunDay | null>(null), error = $state('');
    $effect(() => {
        const abort = new AbortController();
        day = null; error = '';
        client.day(coordinate, date, abort.signal).then(value => { if (!abort.signal.aborted) day = value; }, reason => { if (!abort.signal.aborted) error = reason.message; });
        return () => abort.abort();
    });
    const windows = $derived.by(() => {
        const result: [number, number][] = [];
        day?.states.forEach((value, i) => {
            if (value !== SUN) return;
            if (result.at(-1)?.[1] === i * 5) result[result.length - 1][1] = (i + 1) * 5;
            else result.push([i * 5, (i + 1) * 5]);
        });
        return result;
    });
    const uncertain = $derived.by(() => {
        if (!day?.daylight) return false;
        const { states, daylight } = day;
        return states.some((value, i) => value === UNKNOWN && i * 5 >= daylight[0] && i * 5 < daylight[1]);
    });
    const duration = (minutes: number) => `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
</script>

{#if day}
    <dl>
        <div><dt>First direct sun</dt><dd>{uncertain ? 'Incomplete terrain' : windows.length ? clock(windows[0][0]) : 'No direct sun'}</dd></div>
        <div><dt>Last direct sun</dt><dd>{uncertain ? 'Incomplete terrain' : windows.length ? clock(windows.at(-1)![1]) : 'No direct sun'}</dd></div>
        <div><dt>Direct sun today</dt><dd>{uncertain ? 'Incomplete terrain' : duration(windows.reduce((sum, [a, b]) => sum + b - a, 0))}</dd></div>
        <div><dt>Sunrise / sunset</dt><dd>{day.daylight ? `${clock(day.daylight[0])} / ${clock(day.daylight[1])}` : 'No daylight'}</dd></div>
        <div><dt>Daylight</dt><dd>{day.daylight ? duration(day.daylight[1] - day.daylight[0]) : '0h'}</dd></div>
    </dl>
    <div class="timeline" role="img" aria-label={`Direct sun windows: ${windows.map(([a, b]) => `${clock(a)} to ${clock(b)}`).join(', ') || 'none'}`}>
        <div class="daylight">{#if day.daylight}<span style:left={`${day.daylight[0] / 14.4}%`} style:width={`${(day.daylight[1] - day.daylight[0]) / 14.4}%`}></span>{/if}</div>
        <div class="direct">{#each windows as [a, b]}<span style:left={`${a / 14.4}%`} style:width={`${(b - a) / 14.4}%`}></span>{/each}</div>
        <i style:left={`${minute / 14.4}%`}></i>
    </div>
    <div class="hours"><span>00</span><span>12</span><span>24</span></div>
    {#if onTime && !uncertain && windows.length}<div class="jumps"><button type="button" onclick={() => onTime?.(windows[0][0])}>First sun</button><button type="button" onclick={() => onTime?.(windows.at(-1)![1])}>Last sun</button></div>{/if}
    <p class="note">{windows.map(([a, b]) => `${clock(a)}–${clock(b)}`).join(', ') || 'No direct sun windows.'} · {day.timezone} · estimates in 5-minute steps.</p>
{:else}<p class="note" role="status">{error || 'Checking sun windows…'}</p>{/if}

<style>
    dl { margin: 0; font-variant-numeric: tabular-nums; }
    dl div { display: flex; justify-content: space-between; gap: 12px; margin-top: 6px; }
    dt { color: var(--ink-soft); } dd { margin: 0; font-weight: 600; text-align: right; }
    .timeline { position: relative; margin-top: 14px; }
    .timeline div { height: 8px; position: relative; background: var(--parchment-2); }
    .timeline .direct { margin-top: 4px; }
    .timeline span { position: absolute; height: 8px; background: #8b9faa; }
    .timeline .direct span { background: #e8b352; }
    .timeline i { position: absolute; top: -2px; bottom: -2px; width: 2px; background: var(--ink); }
    .hours { display: flex; justify-content: space-between; margin-top: 4px; font-size: 11px; color: var(--ink-soft); font-variant-numeric: tabular-nums; }
    .jumps { display: flex; gap: 8px; margin-top: 10px; }
    button { border: 1px solid var(--line-strong); border-radius: 6px; padding: 5px 8px; font-weight: 600; }
    button:hover { background: var(--parchment-2); }
    button:focus-visible { outline: 2px solid var(--forest); outline-offset: 2px; }
    .note { margin: 8px 0 0; font-size: 12px; color: var(--ink-soft); }
</style>
