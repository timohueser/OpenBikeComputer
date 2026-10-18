import { expect, it } from "vitest";
import type { RunStep, RunView } from "./shapes";
import { timeline } from "./timeline";

const T0 = Date.parse("2026-10-17T07:00:00Z");
const time = (seconds: number) => new Date(T0 + seconds * 1000).toISOString().replace(".000", "");

function step(name: string, started: number | null, wall_s: number | null, more: Partial<RunStep> = {}): RunStep {
    const wall_ms = wall_s === null ? null : wall_s * 1000;
    return {
        step: name,
        layer: wall_s === null ? "" : "test",
        reused: false,
        unchanged: false,
        started: started === null ? null : time(started),
        wall_ms,
        cpu_ms: wall_ms,
        peak_rss_bytes: null,
        bytes_out: null,
        error: null,
        ...more,
    };
}

function run(state: RunView["state"], steps: RunStep[], wall_s: number | null = null): RunView {
    return {
        id: "2026-10-17-070000",
        args: ["update", "local"],
        started: time(0),
        state,
        wall_ms: wall_s === null ? null : wall_s * 1000,
        bytes_fetched: 0,
        bytes_built: 0,
        phases: [{ phase: "build", state, started: time(0) }],
        layers: [],
        switched: [],
        fetches: [{ source: "hansen-gfc", version: "v1.11", params: [], started: time(0), bytes: 1, wall_ms: 4000, error: null }],
        steps,
        error: null,
        log: [],
        log_path: "",
        review: null,
    };
}

it("a step takes the first row that is free when it starts, so the rows are the most that ran at once", () => {
    const view = run("ok", [step("planner/snow", 10, 100), step("device/osm", 20, 30), step("device/peaks/0037-0032", 60, 5)], 200);
    const { bars, lanes, span } = timeline(view, 0);
    const rows = Object.fromEntries(bars.map((bar) => [bar.name, bar.lane]));
    expect(rows).toEqual({ "hansen-gfc@v1.11": 0, "planner/snow": 0, "device/osm": 1, "device/peaks/0037-0032": 1 });
    expect(lanes).toBe(2);
    expect(span).toBe(200_000);
});

it("only steps that did work in this run are bars, and a step without an end runs until now or ended with the run", () => {
    const steps = [step("device/osm", 10, 30), step("device/fine/0037-0033", 50, null), step("planner/osm", 5, 1, { reused: true }), step("planner/x", null, null, { unchanged: true })];
    const live = timeline(run("running", steps), T0 + 90_000);
    expect(live.bars.map((bar) => [bar.name, bar.state, bar.end])).toEqual([
        ["hansen-gfc@v1.11", "done", 4000],
        ["device/osm", "done", 40_000],
        ["device/fine/0037-0033", "running", 90_000],
    ]);
    expect([live.cpu_ms, live.busy_ms]).toEqual([30_000, 30_000]);

    const died = timeline(run("failed", steps), T0 + 90_000);
    expect(died.bars.at(-1)).toMatchObject({ state: "failed", start: 50_000, end: 50_000 });
});
