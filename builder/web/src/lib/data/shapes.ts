// The JSON of the commands that the page reads. `test-support/data/` has an example of each.

/** What `obc data --json` writes about this machine and each target. The header shows it and
 *  polls it. */
export interface Status {
    machine: Machine;
    targets: Target[];
}

/** A target as `obc data --json` gives it. */
export interface Target {
    target: string;
    kind: "dir" | "r2";
    /** The folder of a dir target, or the public URL of an R2 bucket. */
    at: string;
    /** `null` while it serves nothing. */
    release: { serving: Role; rollback: Role | null } | null;
    lock: Holder | null;
    lock_error: string | null;
    region: string | null;
    /** `null` while the target serves nothing or its steps cannot be listed. */
    layers: Layer[] | null;
    blocked: { layer: string; step: string; reason: string }[];
    /** The pointer keys whose bytes are not those of the serving release. */
    drift: string[];
    error: string | null;
    fix: string | null;
    /** The command to run next, or the run to wait for. */
    next: string | null;
}

/** A release and the time that the target took it. */
export interface Role {
    id: string;
    since: string;
}

/** A layer of a target: its state against the serving release, and what a picture of the layers
 *  needs. `reads` names layers and sources. */
export interface Layer {
    layer: string;
    state: "ok" | "stale" | "off";
    /** Why a stale layer is stale, in a few words. */
    text: string | null;
    shape: "tiled" | "whole" | null;
    format: number | null;
    revision: number | null;
    readers: string[];
    reads: string[];
}

export const LAYER_MARK: Record<Layer["state"], string> = { ok: "✓", stale: "●", off: "○" };

/** What `obc data update TARGET --dry-run --json` writes. */
export interface Review {
    /** Start the update with `--expect ID`: it starts only when the decisions are these. */
    id: string;
    target: string;
    base: string | null;
    settings: { region: string; layers: string[]; refresh: Record<string, number | "manual"> };
    layers: {
        layer: string;
        text: string;
        steps: string[];
        /** The grid leaves of a tiled layer that bake. */
        leaves: string[];
        drops: string[];
        readers: string[];
        format: { from: number; to: number } | null;
    }[];
    sources: { source: string; from: string | null; to: string }[];
    /** The outputs that the release keeps by reference. */
    kept: number;
    fetches: { source: string; version: string; bytes: number | null }[];
    bakes: { step: string }[];
    downloads: { files: number; bytes: number };
    cost: {
        wall_ms: number;
        peak_rss_bytes: number | null;
        store_bytes: number;
        upload_bytes: number;
        unknown: { estimate: string[]; peak: string[]; size: string[] };
    };
    machine: { refusals: string[] };
    after: { rollback: string | null; collects: { release: string; keys: number; bytes: number } | null };
    /** The arguments that start the update of this review. */
    args: string[];
    /** The names that `--add` and `--remove` take; `settings.layers` holds those that are on. */
    optional: string[];
}

/** What `update --json` writes: the run that it started. */
export interface Started {
    run: string;
    review: string;
}

/** The run that holds the lock of a target, from its `lock.json`. A run on another machine shows
 *  only this, read only. */
export interface Holder {
    machine: string;
    run: string;
    phase: Phase;
    /** `YYYY-MM-DDTHH:MM:SSZ`; the run writes it each minute. */
    heartbeat: string;
    /** Each layer that the run bakes, at the last heartbeat. */
    progress: { layer: string; done: number; total: number }[];
    /** Whether the heartbeat is old enough for `unlock`: the run likely ended without freeing the lock. */
    stale: boolean;
}

/** The runs of other machines, from the locks of the targets that they change. */
export function elsewhere(status: Status | null): { target: string; holder: Holder }[] {
    const here = status?.machine.name;
    return (status?.targets ?? []).flatMap(({ target, lock }) => (lock && lock.machine !== here ? [{ target, holder: lock }] : []));
}

/** Whether run `run`, which held the lock of `target`, ended: the lock is free or another run holds
 *  it. A lock that cannot be read says nothing about the run. */
export function ended(status: Status | null, target: string, run: string): boolean {
    const now = status?.targets.find((at) => at.target === target);
    return !!now && now.lock_error === null && now.lock?.run !== run;
}

export const PHASES = ["prepare", "download", "build", "verify", "upload", "switch", "collect"] as const;
export type Phase = (typeof PHASES)[number];

/** `starting` is a run that no process took yet. `failed` is also a run whose process ended before
 *  it finished. */
export type RunState = "starting" | "running" | "ok" | "failed" | "stopped";

export const MARK: Record<RunState, string> = { starting: "◌", running: "◐", ok: "✓", failed: "✗", stopped: "■" };

/** A run as `obc data runs --json` lists it, newest first. */
export interface RunSummary {
    id: string;
    /** The arguments after `obc data` that started it. */
    args: string[];
    started: string;
    state: RunState;
    /** `null` until it ends. */
    wall_ms: number | null;
    bytes_fetched: number;
    bytes_built: number;
}

/** What `obc data runs --json` writes. */
export interface RunList {
    runs: RunSummary[];
}

/** What `obc data runs RUN --json` writes, and `runs RUN --stop`. */
export interface RunView extends RunSummary {
    /** Each phase that it entered, in order. */
    phases: { phase: Phase; state: RunState; started: string }[];
    /** Each layer that it bakes. A tiled layer names its leaves `<i>-<j>`. */
    layers: { layer: string; total: number; done: number; leaves_done: string[]; leaves_baking: string[] }[];
    /** The releases that it switched its target to. */
    switched: string[];
    /** `kept` names the steps whose layers the store keeps for the next update. */
    error: { code: string; message: string; fix: string; kept: string[] } | null;
    /** The last lines of its log. */
    log: string[];
}

/** `colour` is the header background, behind white text. `os` is `linux` or `macos`. A size is
 *  `null` where the system does not tell. */
export interface Machine {
    name: string;
    colour: string;
    os: string;
    cores: number;
    memory_bytes: number | null;
    store_free_bytes: number | null;
}

/** `optional` is a missing thing that no main job needs. */
export type Readiness = "ready" | "missing" | "optional";

/** What `obc data setup --json` writes. It exits 0 also when tools or credentials are missing, so
 *  missing things are output, not an error. It walks the store for `used_bytes`, so it runs only
 *  when the Machine screen opens and on Check again. */
export interface Setup {
    store: { path: string; used_bytes: number; free_bytes: number | null };
    /** What this machine can do, and the reason when it cannot. */
    jobs: { name: string; state: Readiness; reason: string }[];
    /** `detail` is the version found, or the command that installs the tool. */
    tools: { name: string; needed_for: string; state: Readiness; detail: string }[];
    /** Only whether each credential is set; never its value. */
    credentials: { name: string; needed_for: string; state: Readiness }[];
}

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function size(bytes: number): string {
    let unit = 0;
    while (bytes >= 1024 && unit < UNITS.length - 1) {
        bytes /= 1024;
        unit++;
    }
    return `${bytes < 10 && unit > 0 ? bytes.toFixed(1) : Math.round(bytes)} ${UNITS[unit]}`;
}

export function free(bytes: number | null): string {
    return bytes === null ? "free space unknown" : `${size(bytes)} free`;
}

const OS: Record<string, string> = { linux: "Linux", macos: "macOS" };

export function hardware(machine: Machine): string {
    const memory = machine.memory_bytes === null ? [] : [`${size(machine.memory_bytes)} RAM`];
    return [OS[machine.os] ?? machine.os, `${machine.cores} cores`, ...memory].join(" · ");
}

/** `YYYY-MM-DDTHH:MM:SSZ` as the local time of day, `09:31`. */
export function clock(time: string): string {
    return new Date(time).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
}

/** `YYYY-MM-DDTHH:MM:SSZ` as the local day and time, `12 Oct, 09:31`. */
export function when(time: string): string {
    return `${new Date(time).toLocaleDateString([], { day: "numeric", month: "short" })}, ${clock(time)}`;
}

/** `2 h 5 min`, `44 min`, `40 s`. */
export function span(ms: number): string {
    const seconds = Math.max(0, Math.round(ms / 1000));
    if (seconds < 60) return `${seconds} s`;
    const minutes = Math.round(seconds / 60);
    return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** The first 4 hex digits of a release id, as a person reads it. */
export function short(id: string): string {
    return id.slice(0, 4);
}

/** What a person calls a run: `update local` of `update local --region freiburg --expect ID`. */
export function title(args: readonly string[]): string {
    return args.filter((arg) => !arg.startsWith("-")).slice(0, 2).join(" ");
}

/** The target that a run changes: `update` and `rollback` name it first. */
export function changes(args: readonly string[]): string | null {
    return ["update", "rollback"].includes(args[0]) && args[1] ? args[1] : null;
}
