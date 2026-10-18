import type { RunView } from "./shapes";

/** A fetch or a step of a run on its timeline. Times are milliseconds after the start of the run. */
export interface Bar {
    name: string;
    kind: "fetch" | "device" | "planner" | "other";
    state: "done" | "running" | "failed";
    start: number;
    end: number;
    /** The row: the first one that was free when it started. */
    lane: number;
    /** CPU time over wall time; `null` when the run did not record it. */
    cores: number | null;
}

export interface Timeline {
    bars: Bar[];
    /** The number of rows: the most fetches and steps that ran at once. */
    lanes: number;
    /** From the start of the run to its end, or to `now` while it runs. */
    span: number;
    /** The CPU time of the steps that built, and the time from the first of them to the last end. */
    cpu_ms: number;
    busy_ms: number;
}

const kind = (name: string): Bar["kind"] => (name.startsWith("device/") ? "device" : name.startsWith("planner/") ? "planner" : "other");

/** The fetches and steps that `view` ran, each in the first row that is free when it starts. A step
 *  without an end runs until `now` while the run runs; else it ended with the run, at the last time
 *  that the run recorded. Unchanged and reused steps did no work in this run. */
export function timeline(view: RunView, now: number): Timeline {
    const t0 = Date.parse(view.started);
    const live = view.state === "running" || view.state === "starting";
    const at = (time: string) => Date.parse(time) - t0;
    const items = [
        ...view.fetches.flatMap((fetch) =>
            fetch.started ? [{ name: `${fetch.source}@${fetch.version}`, kind: "fetch" as const, start: at(fetch.started), wall: fetch.wall_ms, error: !!fetch.error, cpu: null }] : [],
        ),
        ...view.steps.flatMap((step) =>
            step.started && !step.reused && !step.unchanged
                ? [{ name: step.step, kind: kind(step.step), start: at(step.started), wall: step.wall_ms, error: !!step.error, cpu: step.cpu_ms ?? null }]
                : [],
        ),
    ].sort((a, b) => a.start - b.start);
    const known = [...items.map((item) => item.start + (item.wall ?? 0)), ...view.phases.map((phase) => at(phase.started))];
    const end = live ? now - t0 : view.wall_ms ?? Math.max(0, ...known);
    const free: number[] = [];
    const bars = items.map((item): Bar => {
        const stop = item.wall === null ? Math.max(item.start, end) : item.start + item.wall;
        const state = item.error || (item.wall === null && !live) ? "failed" : item.wall === null ? "running" : "done";
        let lane = free.findIndex((until) => until <= item.start);
        if (lane < 0) lane = free.push(stop) - 1;
        else free[lane] = stop;
        const cores = item.cpu !== null && item.wall ? item.cpu / item.wall : null;
        return { name: item.name, kind: item.kind, state, start: item.start, end: stop, lane, cores };
    });
    const built = items.filter((item) => item.cpu !== null && item.wall !== null);
    const first = Math.min(...built.map((item) => item.start));
    const last = Math.max(...built.map((item) => item.start + item.wall!));
    return {
        bars,
        lanes: free.length,
        span: Math.max(end, ...bars.map((bar) => bar.end)),
        cpu_ms: built.reduce((sum, item) => sum + item.cpu!, 0),
        busy_ms: built.length ? last - first : 0,
    };
}
