/**
 * One device write, followed on screen: phase, bytes, throughput, an honest estimate, and a Cancel
 * that actually reaches the transport.
 *
 * All three of C4's flows run through this, because the interesting behaviour is identical in all
 * three and only one of them is short. A map is hundreds of megabytes, and no stage of the pipeline
 * that carries it is fast enough to make that instant, so a large map is *minutes* rather than "USB
 * is fast". A progress bar that only counts percent invites the rider to think
 * something is stuck at 12%; a rate and a remaining time say what is actually happening.
 *
 * The rate is measured over a short trailing window rather than since the start, because the two
 * phases of a transfer can run at different speeds. An average over the whole job can therefore
 * spend the first minute predicting a finish that never arrives.
 */

import type { JobContext, JobPhase } from "./progress";
import { deviceHolder } from "./session.svelte";

export type { JobContext, JobPhase };

/** How long a window the throughput estimate looks back over. */
const RATE_WINDOW_MS = 4_000;

/**
 * A single job slot.
 *
 * One per surface (map, route, firmware), so the three can be described independently — but the
 * protocol client allows exactly one transfer at a time, and a second one is answered `busy` by the
 * device rather than interleaved. That is the client's rule to enforce (§4.1) and this class does
 * not duplicate it; what it does guarantee is that *this* slot never runs two tasks at once.
 */
export class DeviceJob {
    /** What this slot writes, for chrome that reports a transfer it does not own — one word,
     *  lowercase, reading naturally after "sending": "map", "route", "firmware", "rides". */
    readonly label: string;

    constructor(label = "transfer") {
        this.label = label;
    }

    phase = $state<JobPhase>("idle");
    done = $state(0);
    total = $state(0);
    error = $state<string | null>(null);
    /** The failure's stable code where it had one — `DeviceError.code`,
     *  `ConvertError.code`. The message is for the rider; this is for the caller, which has to
     *  tell "the cable came out" from "the card is full" without reading English. */
    errorCode = $state<string | null>(null);
    /** A sentence for the successful case — what was written, and what it means. */
    result = $state<string | null>(null);
    /** Bytes per second over the last few seconds, or null before there is enough to say. */
    rate = $state<number | null>(null);

    /** `$state` because {@link running} is read from the markup — a plain field would leave the
     *  progress bar and the disabled buttons frozen at whatever they were on first render. */
    private controller = $state<AbortController | null>(null);
    /** `[timestamp, bytes]` samples inside the rate window. */
    private samples: Array<[number, number]> = [];

    get running(): boolean {
        return this.controller !== null;
    }

    get pct(): number {
        return this.total > 0 ? Math.min(100, Math.round((this.done / this.total) * 100)) : 0;
    }

    /** Seconds left at the current rate, or null when there is nothing honest to say yet. */
    get etaSeconds(): number | null {
        if (!this.rate || this.total <= 0 || this.done >= this.total) return null;
        return (this.total - this.done) / this.rate;
    }

    /**
     * Run `task`, holding this slot until it settles.
     *
     * Returns the task's value, or `null` if it failed or was cancelled — a caller that only wants
     * to render the outcome reads {@link error} and never has to catch. A cancel is not an error:
     * the job returns to `idle` with nothing said, because the rider already knows what they did.
     */
    async run<T>(task: (ctx: JobContext) => Promise<T>, describe: (value: T) => string): Promise<T | null> {
        if (this.controller) return null;
        const controller = new AbortController();
        this.controller = controller;
        jobRegistry.add(this);
        this.error = null;
        this.errorCode = null;
        this.result = null;
        this.done = 0;
        this.total = 0;
        this.rate = null;
        this.samples = [];
        this.phase = "reading";
        try {
            const value = await task({
                signal: controller.signal,
                cancel: (reason) =>
                    controller.abort(reason ?? new DOMException("cancelled by the rider", "AbortError")),
                phase: (phase, total) => {
                    this.phase = phase;
                    if (total !== undefined) {
                        this.total = total;
                        this.done = 0;
                    }
                    this.samples = [];
                    this.rate = null;
                },
                progress: (done, total) => this.sample(done, total),
            });
            this.phase = "done";
            this.done = this.total;
            this.result = describe(value);
            return value;
        } catch (cause) {
            const code = (cause as { code?: unknown })?.code;
            const errorCode = typeof code === "string" ? code : null;
            // A physical disconnect can race the surface teardown that aborts this job. The
            // transport's stable link cause is stronger evidence than the later local abort.
            if (errorCode === "link") {
                this.errorCode = errorCode;
                this.phase = "error";
                this.error = cause instanceof Error ? cause.message : String(cause);
                deviceHolder.noteInterrupted();
            } else if (controller.signal.aborted) {
                this.phase = "idle";
                this.done = 0;
                this.total = 0;
            } else {
                this.phase = "error";
                this.error = cause instanceof Error ? cause.message : String(cause);
                this.errorCode = errorCode;
            }
            return null;
        } finally {
            this.controller = null;
            this.rate = null;
            jobRegistry.remove(this);
        }
    }

    /** Cancel the running task. Safe to call when nothing is running. */
    cancel(): void {
        // `finalizing` starts only after the device's commit is durable. There is no transfer left
        // to cancel, and aborting now would let surface teardown mislabel success as cancellation.
        if (this.phase === "finalizing") return;
        this.controller?.abort(new DOMException("cancelled by the rider", "AbortError"));
    }

    /** Clear a finished job's outcome, so the surface goes back to offering the action. */
    reset(): void {
        if (this.running) return;
        this.phase = "idle";
        this.done = 0;
        this.total = 0;
        this.error = null;
        this.result = null;
    }

    private sample(done: number, total: number): void {
        this.done = done;
        this.total = total;
        const now = Date.now();
        this.samples.push([now, done]);
        while (this.samples.length > 1 && now - this.samples[0][0] > RATE_WINDOW_MS) this.samples.shift();
        const [firstAt, firstBytes] = this.samples[0];
        const elapsed = now - firstAt;
        // Under a second of history says nothing useful, and a rate that jumps around is worse than
        // no rate at all — the number is there to set an expectation, not to be watched.
        this.rate = elapsed >= 1_000 ? ((done - firstBytes) * 1000) / elapsed : this.rate;
    }
}

/**
 * Every job that is running right now, so chrome outside the surfaces — the header's device chip —
 * can report a transfer without owning it. Jobs register per *run*, not per instance: an idle
 * `DeviceJob` created in a component's script block is invisible here, and one abandoned by an
 * unmount mid-run still unregisters, because `run()`'s `finally` is what removes it.
 *
 * A list rather than a single slot, deliberately: the client allows one *transfer* at a time, but a
 * job may briefly overlap another during its non-transfer phases (a map send filling its scratch
 * file while a pull settles). First registered wins the readout; precision beyond that buys nothing.
 */
class JobRegistry {
    private jobs = $state<DeviceJob[]>([]);

    add(job: DeviceJob): void {
        this.jobs = [...this.jobs, job];
    }

    remove(job: DeviceJob): void {
        this.jobs = this.jobs.filter((j) => j !== job);
    }

    /** The job the readout shows, or null when nothing is running. */
    get active(): DeviceJob | null {
        return this.jobs[0] ?? null;
    }
}

export const jobRegistry = new JobRegistry();
