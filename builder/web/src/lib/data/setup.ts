// What `obc data setup --json` writes. `test-support/data/setup.json` is an example of it.

/** This machine. `colour` is the header background, so it is dark enough for white text. */
export interface Machine {
    name: string;
    colour: string;
    os: string;
    cores: number;
    memory_bytes: number;
}

/** `optional` is a missing thing that no main job needs. */
export type Readiness = "ready" | "missing" | "optional";

export interface Setup {
    machine: Machine;
    store: { path: string; used_bytes: number; free_bytes: number };
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

export function hardware(machine: Machine): string {
    return `${machine.os} · ${machine.cores} cores · ${size(machine.memory_bytes)} RAM`;
}
