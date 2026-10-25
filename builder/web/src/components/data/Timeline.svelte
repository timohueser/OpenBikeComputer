<script lang="ts">
    import { clock, span, type RunView } from "../../lib/data/shapes";
    import { timeline, type Bar } from "../../lib/data/timeline";

    // Where the time of a run went: one row for each download and step that did work, on one clock
    // from the start of the run to its end, or to now while it runs. A long run shows its slowest rows.
    let { view, now }: { view: RunView; now: number } = $props();

    const ROWS = 30;
    const EVERY = [1, 2, 5, 10, 15, 30, 60, 120, 180, 360, 720].map((minutes) => minutes * 60_000);

    const t = $derived(timeline(view, now));
    const t0 = $derived(Date.parse(view.started));
    const live = $derived(view.state === "running" || view.state === "starting");
    const shown = $derived.by(() => {
        if (t.bars.length <= ROWS) return t.bars;
        const slowest = new Set([...t.bars].sort((a, b) => b.end - b.start - (a.end - a.start)).slice(0, ROWS));
        return t.bars.filter((bar) => slowest.has(bar));
    });
    const at = (ms: number) => clock(new Date(t0 + ms).toISOString());
    const pct = (ms: number) => `${(ms / Math.max(t.span, 1)) * 100}%`;
    // About five clock times along the track, on whole minutes or hours.
    const ticks = $derived.by(() => {
        const every = EVERY.find((step) => t.span / step <= 5) ?? Math.ceil(t.span / 5 / 3_600_000) * 3_600_000;
        const local = -new Date(t0).getTimezoneOffset() * 60_000;
        const first = Math.ceil((t0 + local) / every) * every - local - t0;
        return Array.from({ length: Math.max(0, Math.floor((t.span - first) / every) + 1) }, (_, i) => first + i * every);
    });
    const KIND = (bar: Bar) => (bar.kind === "fetch" ? "Download" : "Step");
    const total = (kind: "fetch" | "step") => {
        const bars = t.bars.filter((bar) => (bar.kind === "fetch") === (kind === "fetch"));
        return { count: bars.length, ms: bars.reduce((sum, bar) => sum + bar.end - bar.start, 0) };
    };
    const summary = $derived.by(() => {
        const [downloads, steps] = [total("fetch"), total("step")];
        const parts = [`${span(t.span)}${live ? " so far" : ""}`];
        if (downloads.count) parts.push(`${downloads.count} ${downloads.count === 1 ? "download" : "downloads"}, ${span(downloads.ms)}`);
        if (steps.count) parts.push(`${steps.count} ${steps.count === 1 ? "step" : "steps"}, ${span(steps.ms)}`);
        if (t.lanes > 1) parts.push(`at most ${t.lanes} at once`);
        return parts.join(" · ");
    });
    const legend = $derived([
        ...(t.bars.some((bar) => bar.kind === "fetch" && bar.state === "done") ? [["fetch", "Download"]] : []),
        ...(t.bars.some((bar) => bar.kind !== "fetch" && bar.state === "done") ? [["step", "Step"]] : []),
        ...(t.bars.some((bar) => bar.state === "running") ? [["running", "Running now"]] : []),
        ...(t.bars.some((bar) => bar.state === "failed") ? [["failed", "Failed"]] : []),
    ]);
    const tone = (bar: Bar) => (bar.state === "done" ? (bar.kind === "fetch" ? "fetch" : "step") : bar.state);
</script>

{#if t.bars.length}
    <section class="card" aria-labelledby="time">
        <div class="title">
            <h2 id="time">Where the time went</h2>
            <span class="soft">{summary}</span>
        </div>
        <ul class="legend" aria-label="Colours">
            {#each legend as [key, label] (key)}<li><span class="swatch {key}"></span>{label}</li>{/each}
        </ul>
        <div class="chart" style:--rows={shown.length + 1}>
            <div class="clock" aria-hidden="true">
                {#each ticks as tick (tick)}
                    <span class="tick" style:left={pct(tick)}><span>{at(tick)}</span></span>
                {/each}
            </div>
            <span class="head">From {at(0)}</span>
            <span class="head end">Took</span>
            {#each shown as bar, i (i)}
                {@const took = span(bar.end - bar.start)}
                <div class="row" style:--row={i + 2} title="{KIND(bar)} {bar.name} · {at(bar.start)}–{at(bar.end)} · {took}{bar.state === 'done' ? '' : ` · ${bar.state}`}">
                    <span class="name"><span class="kind">{KIND(bar)}</span> {bar.name}</span>
                    <span class="track">
                        <span class="bar {tone(bar)}" style:left={pct(bar.start)} style:width="max(3px, {pct(bar.end - bar.start)})"></span>
                    </span>
                    <span class="took">{took}{bar.state === "running" ? " so far" : bar.state === "failed" ? ", failed" : ""}</span>
                </div>
            {/each}
        </div>
        {#if shown.length < t.bars.length}
            <p class="soft">The {ROWS} slowest of {t.bars.length}; the steps table lists all of them.</p>
        {/if}
    </section>
{/if}

<style>
    .card {
        --fetch: #eb6834;
        --step: #1baf7a;
    }
    .legend {
        display: flex;
        flex-wrap: wrap;
        gap: 4px 16px;
        margin: 0;
        padding: 0;
        list-style: none;
        font-size: 13px;
        color: var(--ink-soft);
    }
    .legend li {
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }
    .swatch {
        width: 10px;
        height: 10px;
        border-radius: 2px;
    }
    .chart {
        display: grid;
        grid-template-columns: minmax(0, 16rem) minmax(0, 1fr) 7rem;
        column-gap: 12px;
        position: relative;
    }
    .clock {
        grid-column: 2;
        grid-row: 1 / span var(--rows);
        position: relative;
        z-index: 1;
        pointer-events: none;
    }
    .tick {
        position: absolute;
        top: 0;
        bottom: 0;
        border-left: 1px solid var(--line);
    }
    .tick span {
        position: absolute;
        top: 0;
        left: 4px;
        font-size: 12px;
        color: var(--ink-soft);
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
    }
    .head {
        grid-column: 1;
        grid-row: 1;
        font-size: 12px;
        color: var(--ink-soft);
    }
    .head.end {
        grid-column: 3;
        text-align: right;
    }
    .row {
        display: contents;
    }
    .row > * {
        grid-row: var(--row);
        padding: 3px 0;
        border-top: 1px solid var(--line);
    }
    .row:hover > * {
        background: var(--parchment);
    }
    .name {
        grid-column: 1;
        overflow: hidden;
        white-space: nowrap;
        text-overflow: ellipsis;
        font: 13px/20px var(--mono);
    }
    .kind {
        font: 600 12px var(--sans);
        color: var(--ink-soft);
    }
    .track {
        grid-column: 2;
        position: relative;
    }
    .bar {
        position: absolute;
        top: 7px;
        height: 10px;
        border-radius: 3px;
    }
    .took {
        grid-column: 3;
        text-align: right;
        font-size: 13px;
        line-height: 20px;
        font-variant-numeric: tabular-nums;
    }
    .fetch {
        background: var(--fetch);
    }
    .step {
        background: var(--step);
    }
    .running {
        background: var(--running);
    }
    .failed {
        background: var(--missing);
    }
    @media (max-width: 640px) {
        .chart {
            grid-template-columns: minmax(0, 8rem) minmax(0, 1fr) 5.5rem;
        }
    }
</style>
