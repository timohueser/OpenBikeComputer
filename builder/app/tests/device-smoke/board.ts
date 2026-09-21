/**
 * The reboot, and what the firmware says while it comes up.
 *
 * `tools/board.py` is the repository's one serialized probe session — it holds the board lock and it
 * is what the Cargo runner and every `obc` device task call — so the run starts one of those and
 * nothing else. The action is `run --preverify`: probe-rs reads the image back, programs nothing
 * when it already matches, resets the chip and then streams RTT. That is the only supported command
 * that both restarts the board and is attached in time to see it boot; `attach` never resets, and a
 * `reset` followed by an `attach` loses the boot lines into the RTT ring.
 *
 * The session is stopped as soon as the three lines are there. It is spawned into its own process
 * group and interrupted through the group, because `board.py` hands the lock to probe-rs and
 * killing only the wrapper would leave the board locked.
 */

import { spawn, type ChildProcess } from "node:child_process";
import { join } from "node:path";

import { bootFault, parseBootLog, type BootObservation } from "../../test-support/device-smoke/smoke";

/** What the run records about the image that was running when the observation was made. */
export interface FirmwareIdentity {
    readonly elf: string;
    readonly sha256: string;
}

export interface BoardOptions {
    readonly repoRoot: string;
    /** The exact ELF of the running image. A mismatched one decodes to noise. */
    readonly elf: string;
    readonly probe?: string;
}

export interface Reboot {
    readonly observation: BootObservation;
    readonly firmware: FirmwareIdentity;
    /** Everything probe-rs and `board.py` printed, kept for the result record. */
    readonly log: string;
}

/** Restart the board and resolve once the firmware has said what it opened. */
export function reboot(options: BoardOptions, signal: AbortSignal): Promise<Reboot> {
    const argv = [join(options.repoRoot, "tools", "board.py"), "run", options.elf, "--preverify"];
    if (options.probe) argv.push("--probe", options.probe);
    const child = spawn("python3", argv, { cwd: options.repoRoot, detached: true, stdio: ["ignore", "pipe", "pipe"] });

    return new Promise<Reboot>((resolve, reject) => {
        let log = "";
        let settled = false;
        const finish = (outcome: () => void) => {
            if (settled) return;
            settled = true;
            signal.removeEventListener("abort", onAbort);
            stop(child);
            outcome();
        };
        const onAbort = () => finish(() => reject(signal.reason));
        signal.addEventListener("abort", onAbort, { once: true });

        const take = (chunk: Buffer) => {
            log += chunk.toString("utf8");
            const fault = bootFault(log);
            if (fault) {
                finish(() => reject(new Error(`The firmware refused the card or the map: ${fault}`)));
                return;
            }
            const observation = parseBootLog(log);
            if (observation) finish(() => resolve({ observation, firmware: firmwareOf(log, options.elf), log }));
        };
        child.stdout?.on("data", take);
        child.stderr?.on("data", take);
        child.on("error", (cause) => finish(() => reject(cause)));
        child.on("exit", (code) =>
            finish(() =>
                reject(new Error(`board.py exited with ${code} before the firmware finished booting.\n${log}`)),
            ),
        );
    });
}

/** `board.py` prints the ELF it was given and that file's digest before it starts probe-rs. */
function firmwareOf(log: string, elf: string): FirmwareIdentity {
    const digest = /^SHA-256: ([0-9a-f]{64})$/m.exec(log);
    return { elf, sha256: digest ? digest[1] : "" };
}

/**
 * Interrupt the whole group, the way Ctrl-C ends an RTT session. `board.py` gives probe-rs the lock
 * descriptor, so the lock is released when probe-rs itself exits, not when the wrapper does.
 */
function stop(child: ChildProcess): void {
    if (child.pid === undefined || child.exitCode !== null) return;
    try {
        process.kill(-child.pid, "SIGINT");
    } catch {
        // The group is already gone, which is the outcome this wanted.
    }
}
