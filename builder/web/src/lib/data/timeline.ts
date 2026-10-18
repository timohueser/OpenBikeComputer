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
}

export interface Timeline {
    bars: Bar[];
    /** The number of rows: the most fetches and steps that ran at once. */
    lanes: number;
    /** From the start of the run to its end, or to `now` while it runs. */
    span: number;
}

const kind = (name: string): Bar["kind"] => (name.startsWith("device/") ? "device" : name.startsWith("planner/") ? "planner" : "other");

/** The fetches and steps that `view` ran, each in the first row that is free when it starts. One that
 *  did not end runs until `now` while the run runs; else it ended with the run, at the last time
 *  that the run recorded. Unchanged and reused steps did no work in this run. */
export function timeline(view: RunView, now: number): Timeline {
    const live = view.state === "running" || view.state === "starting";
    const items = [
        ...view.fetches.map((fetch) => ({ ...fetch, name: `${fetch.source}@${fetch.version}`, kind: "fetch" as const })),
        ...view.steps.filter((step) => !step.reused && !step.unchanged).map((step) => ({ ...step, name: step.step, kind: kind(step.step) })),
    ]
        .flatMap((item) => (item.start_ms == null ? [] : [{ ...item, start: item.start_ms, stop: item.end_ms ?? null }]))
        .sort((a, b) => a.start - b.start);
    // A large run has tens of thousands of steps: no spread into Math.max.
    let last = view.wall_ms ?? 0;
    for (const item of items) last = Math.max(last, item.stop ?? item.start);
    for (const phase of view.phases) last = Math.max(last, Date.parse(phase.started) - Date.parse(view.started));
    const end = live ? Math.max(last, now - Date.parse(view.started)) : last;
    const free: number[] = [];
    const bars = items.map((item): Bar => {
        const stop = item.stop ?? Math.max(item.start, end);
        const state = item.error || (item.stop === null && !live) ? "failed" : item.stop === null ? "running" : "done";
        let lane = free.findIndex((until) => until <= item.start);
        if (lane < 0) lane = free.push(stop) - 1;
        else free[lane] = stop;
        return { name: item.name, kind: item.kind, state, start: item.start, end: stop, lane };
    });
    return { bars, lanes: free.length, span: end };
}
