// The JSON of the commands that the page reads. `test-support/data/` has an example of each.

/** What `obc data --json` writes about this machine. The header shows it and polls it. */
export interface Status {
    machine: Machine;
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
