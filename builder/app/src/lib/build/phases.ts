// The coarse build phases, and the percentage the progress bar derives from
// them. Shared by every host that runs a build, which is the point: the dev
// server scrapes these names off `obc-pack`'s stdout and the desktop app is
// handed them as values (`obc_pack::progress::Phase`), but a build looks the
// same either way, so the arithmetic lives in one place rather than once per
// transport.
//
// Order is the whole mechanism — a phase's *index* is its progress — so this
// array is the same list, in the same order, as `Phase::ALL` in
// host/obc-pack/src/progress.rs and `_STAGE_MARKERS` in
// builder/server/jobs.py. `obc-pack`'s
// `stage_lines_still_match_the_web_builders_markers` is what fails if they drift.
//
// "downloading" leads and has no counterpart in the packer: fetching the `.pbf`
// is the host's own work, before the packer is handed anything.

export const PHASES = [
    "downloading",
    "merging",
    "ingest",
    "bbox",
    "land",
    "quadtree",
    "serialize",
] as const;

export type Phase = (typeof PHASES)[number];

/** One more slot than there are phases, so "serialize" is not 100% — the build
 *  isn't done until the file is. */
const SLOTS = PHASES.length + 1;

/** The bar position for a phase, or null if that isn't a phase name (the UI
 *  then shows the raw detail text instead of moving the bar). */
export function phasePct(detail: string): number | null {
    const i = (PHASES as readonly string[]).indexOf(detail);
    return i < 0 ? null : Math.round(((i + 1) / SLOTS) * 100);
}

/** A download's own percentage, scaled into the first phase's slot. */
export function downloadPct(pct: number): number {
    return Math.round((pct / 100) * (100 / SLOTS));
}
