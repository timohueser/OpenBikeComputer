<script lang="ts">
    import { clock, span, type RunView } from "../../lib/data/shapes";
    import { timeline, type Bar } from "../../lib/data/timeline";

    // The run on a clock: its phases, then one row for each fetch or step that ran beside another.
    // A run that bakes one step at a time has one row.
    let { view, now }: { view: RunView; now: number } = $props();

    const ROW = 14;
    const GAP = 2;
    const BAND = 26;
    const AXIS = 22;
    const PAD = 1;
    const LABEL = 6.4; // px per character of a bar label
    const EVERY = [1, 2, 5, 10, 15, 30, 60, 120, 180, 360, 720].map((minutes) => minutes * 60_000);
    const COLOUR: Record<Bar["kind"], string> = {
        fetch: "var(--amber)",
        device: "var(--water)",
        planner: "var(--route)",
        other: "var(--ink-faint)",
    };
    const KIND: Record<Bar["kind"], string> = { fetch: "fetch", device: "device step", planner: "planner step", other: "other step" };

    let width = $state(0);
    const t = $derived(timeline(view, now));
    const t0 = $derived(Date.parse(view.started));
    const height = $derived(BAND + Math.max(t.lanes, 1) * (ROW + GAP) + AXIS);
    const x = (ms: number) => PAD + (ms / Math.max(t.span, 1)) * Math.max(width - 2 * PAD, 1);
    // Ticks on whole minutes or hours of the local clock, at least 64 px apart.
    const ticks = $derived.by(() => {
        const every = EVERY.find((step) => (t.span / step) * 64 <= width) ?? Math.ceil((t.span * 64) / Math.max(width, 1) / 3_600_000) * 3_600_000;
        const local = -new Date(t0).getTimezoneOffset() * 60_000;
        const first = Math.ceil((t0 + local) / every) * every - local - t0;
        return Array.from({ length: Math.max(0, Math.floor((t.span - first) / every) + 1) }, (_, i) => first + i * every);
    });
    const phases = $derived(
        view.phases.map((phase, i) => ({
            ...phase,
            start: Date.parse(phase.started) - t0,
            end: i + 1 < view.phases.length ? Date.parse(view.phases[i + 1].started) - t0 : t.span,
        })),
    );
    const at = (ms: number) => clock(new Date(t0 + ms).toISOString());
    const fill = (bar: Bar) => (bar.state === "running" ? "var(--running)" : bar.state === "failed" ? "var(--missing)" : COLOUR[bar.kind]);
    const tip = (bar: Bar) => `${bar.name} · ${at(bar.start)}–${at(bar.end)} · ${span(bar.end - bar.start)}${bar.state === "done" ? "" : ` · ${bar.state}`}`;
    /** The longest end of `name`, in whole path parts, that fits in `px`, such as `0037-0033`. */
    const fit = (name: string, px: number) =>
        name
            .split("/")
            .map((_, i, parts) => parts.slice(i).join("/"))
            .find((label) => label.length * LABEL + 8 <= px);
    const steps = $derived(t.bars.filter((bar) => bar.kind !== "fetch").length);
    const summary = $derived(`${steps} ${steps === 1 ? "step" : "steps"} · at most ${t.lanes} at a time`);
    const legend = $derived([
        ...(["device", "planner", "other", "fetch"] as const).filter((kind) => t.bars.some((bar) => bar.kind === kind && bar.state === "done")).map((kind) => ({ label: KIND[kind], colour: COLOUR[kind] })),
        ...(t.bars.some((bar) => bar.state === "running") ? [{ label: "running", colour: "var(--running)" }] : []),
        ...(t.bars.some((bar) => bar.state === "failed") ? [{ label: "failed", colour: "var(--missing)" }] : []),
    ]);
</script>

{#if t.bars.length}
    <section class="card" aria-labelledby="timeline">
        <div class="title">
            <h2 id="timeline">Timeline</h2>
            <span class="soft">{summary}</span>
        </div>
        {#if legend.length}
            <ul class="legend">
                {#each legend as { label, colour } (label)}<li><span class="swatch" style:background={colour}></span>{label}</li>{/each}
            </ul>
        {/if}
        <div class="plot" bind:clientWidth={width}>
            {#if width}
                <svg {width} {height} role="img" aria-label="{view.phases.length} phases and {summary}, from {at(0)} to {at(t.span)}">
                    {#each ticks as tick (tick)}
                        <line class="tick" x1={x(tick)} x2={x(tick)} y1={BAND - 4} y2={height - AXIS + 4} />
                        <text class="tick-label" x={x(tick)} y={height - 6} text-anchor={x(tick) < 24 ? "start" : "middle"}>{at(tick)}</text>
                    {/each}
                    {#each phases as phase (phase.phase)}
                        {@const w = Math.max(x(phase.end) - x(phase.start) - GAP, 1)}
                        <rect class="phase" class:current={phase.state === "running"} x={x(phase.start)} y="2" width={w} height={BAND - 8} rx="3">
                            <title>{phase.phase} · {at(phase.start)} · {span(phase.end - phase.start)}{phase.waiting ? ` · ${phase.waiting}` : ""}</title>
                        </rect>
                        {#if w > phase.phase.length * LABEL + 8}
                            <text class="phase-label" x={x(phase.start) + 5} y={BAND / 2 + 1}>{phase.phase}</text>
                        {/if}
                    {/each}
                    {#each t.bars as bar, i (i)}
                        {@const w = Math.max(x(bar.end) - x(bar.start) - (x(bar.end) - x(bar.start) > 3 ? GAP : 0), 1.5)}
                        {@const y = BAND + bar.lane * (ROW + GAP)}
                        <rect x={x(bar.start)} {y} width={w} height={ROW} rx="2" fill={fill(bar)}><title>{tip(bar)}</title></rect>
                        {@const label = fit(bar.name, w)}
                        {#if label}<text class="bar-label" x={x(bar.start) + 4} y={y + ROW - 3.5}>{label}</text>{/if}
                    {/each}
                </svg>
            {/if}
        </div>
    </section>
{/if}

<style>
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
    .plot {
        min-width: 0;
        overflow: hidden;
    }
    svg {
        display: block;
    }
    .phase {
        fill: var(--parchment-2);
        stroke: var(--line-strong);
    }
    .phase.current {
        stroke: var(--running);
    }
    .phase-label,
    .tick-label {
        font: 12px var(--sans);
        fill: var(--ink-soft);
        dominant-baseline: middle;
        font-variant-numeric: tabular-nums;
    }
    .tick-label {
        dominant-baseline: auto;
    }
    .tick {
        stroke: var(--line);
    }
    .bar-label {
        font: 600 11px var(--sans);
        fill: var(--panel);
        pointer-events: none;
    }
</style>
