/**
 * One device write, followed on screen: phase, bytes, throughput, an honest estimate, and a Cancel
 * that actually reaches the transport.
 *
 * All three of C4's flows run through this, because the interesting behaviour is identical in all
 * three and only one of them is short. A map is hundreds of megabytes over a link whose ceiling is
 * the **SD card** — the proven ~8 MHz SPI to the card is high-hundreds of KB/s, so a regional map is
 * *minutes*, not "USB is fast". A progress bar that only counts percent invites the rider to think
 * something is stuck at 12%; a rate and a remaining time say what is actually happening.
 *
 * The rate is measured over a short trailing window rather than since the start, because the two
 * halves of a map upload run at completely different speeds: the CDN fills the scratch file at
 * network speed, then the card drains it at card speed. An average over the whole job would spend
 * the first minute of the send predicting a finish that never arrives.
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
    phase = $state<JobPhase>("idle");
    done = $state(0);
    total = $state(0);
    error = $state<string | null>(null);
    /** The failure's stable code where it had one — `DeviceError.code`, `StagingError.code`,
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
            if (controller.signal.aborted) {
                this.phase = "idle";
                this.done = 0;
                this.total = 0;
            } else {
                this.phase = "error";
                this.error = cause instanceof Error ? cause.message : String(cause);
                const code = (cause as { code?: unknown })?.code;
                this.errorCode = typeof code === "string" ? code : null;
                // A write killed by the cable coming out has to be reported somewhere that
                // outlives this job: the surface rendering it unmounts the instant the device
                // goes, so its own message would never be read. Handled here rather than at each
                // of the three call sites, so a fourth surface cannot forget to.
                if (this.errorCode === "link") deviceHolder.noteInterrupted();
            }
            return null;
        } finally {
            this.controller = null;
            this.rate = null;
        }
    }

    /** Cancel the running task. Safe to call when nothing is running. */
    cancel(): void {
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
