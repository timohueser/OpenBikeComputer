import { expect, it } from "vitest";
import type { RunStep, RunView } from "./shapes";
import { timeline } from "./timeline";

const T0 = Date.parse("2026-10-17T07:00:00Z");

/** A step that started at `start` ms and ended at `end` ms after the start of the run. */
function step(name: string, start: number | null, end: number | null, more: Partial<RunStep> = {}): RunStep {
    return {
        step: name,
        layer: end === null ? "" : "test",
        reused: false,
        unchanged: false,
        start_ms: start,
        end_ms: end,
        wall_ms: start !== null && end !== null ? end - start : null,
        peak_rss_bytes: null,
        bytes_out: null,
        error: null,
        ...more,
    };
}

function run(state: RunView["state"], steps: RunStep[], wall_ms: number | null = null): RunView {
    return {
        id: "2026-10-17-070000",
        args: ["update", "local"],
        started: new Date(T0).toISOString(),
        state,
        wall_ms,
        bytes_fetched: 0,
        bytes_built: 0,
        phases: [{ phase: "build", state, started: new Date(T0).toISOString() }],
        layers: [],
        switched: [],
        fetches: [{ source: "hansen-gfc", version: "v1.11", params: [], start_ms: 0, end_ms: 4000, bytes: 1, wall_ms: 4000, error: null }],
        steps,
        error: null,
        log: [],
        log_path: "",
        review: null,
    };
}

it("a step takes the first row that is free when it starts, so the rows are the most that ran at once", () => {
    // device/peaks starts 100 ms after device/osm ended: one step at a time in that row.
    const view = run("ok", [step("planner/snow", 10_000, 110_000), step("device/osm", 20_000, 25_500), step("device/peaks/0037-0032", 25_600, 30_000)], 200_000);
    const { bars, lanes, span } = timeline(view, 0);
    const rows = Object.fromEntries(bars.map((bar) => [bar.name, bar.lane]));
    expect(rows).toEqual({ "hansen-gfc@v1.11": 0, "planner/snow": 0, "device/osm": 1, "device/peaks/0037-0032": 1 });
    expect(lanes).toBe(2);
    expect(span).toBe(200_000);
});

it("a failed step ends when it failed, a step without an end runs until now or ended with the run, and steps without work are not bars", () => {
    const steps = [
        step("device/osm", 10_000, 40_000),
        step("device/coarse/0037-0032", 12_000, 15_000, { error: "boom" }),
        step("device/fine/0037-0033", 50_000, null),
        step("planner/osm", 5000, 5001, { reused: true }),
        step("planner/x", null, null, { unchanged: true }),
    ];
    const live = timeline(run("running", steps), T0 + 90_000);
    expect(live.bars.map((bar) => [bar.name, bar.state, bar.end])).toEqual([
        ["hansen-gfc@v1.11", "done", 4000],
        ["device/osm", "done", 40_000],
        ["device/coarse/0037-0032", "failed", 15_000],
        ["device/fine/0037-0033", "running", 90_000],
    ]);
    const died = timeline(run("failed", steps), T0 + 90_000);
    expect(died.bars.at(-1)).toMatchObject({ state: "failed", start: 50_000, end: 50_000 });
});

it("a run of 100,000 steps has a timeline", () => {
    const steps = Array.from({ length: 100_000 }, (_, i) => step(`device/terrain/${i}`, 4000 + i * 10, 4000 + i * 10 + 5));
    expect(timeline(run("ok", steps, 1_000_000), 0).lanes).toBe(1);
});
