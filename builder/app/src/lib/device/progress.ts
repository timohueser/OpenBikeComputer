/**
 * What a long device write reports while it runs — the seam between the flows (`write.ts`, plain
 * TypeScript, tested under Node) and the thing that renders them (`job.svelte.ts`, runes).
 *
 * It lives in its own file so the flows never import a `.svelte.ts` module: everything with logic
 * in it stays framework-free and reusable by the desktop app, which is the same split C3 drew
 * between its client and its reactive session shell.
 */

/**
 * Where a job is.
 *
 * `downloading` and `sending` are deliberately distinct: they run at different speeds, they fail
 * for different reasons, and which of them dominates is not fixed — a CDN fills a scratch file at
 * whatever the rider's line does, and the device drains it at whatever the upload pipeline manages.
 * Both are minutes for a regional map; neither is reliably the longer one.
 *
 * `downloading` covers both directions a byte can arrive from — a CDN, or the device itself when a
 * ride is pulled (C5 #904). `converting` is the ride export's second half: the wasm exporter turning
 * the pulled object into GPX, which moves no bytes over the cable and so deserves its own word
 * rather than a progress bar that looks stalled at 100%.
 *
 * `committing` is the same argument at the other end of a write: the last byte is on the wire and
 * the device is now closing the file, validating it and making it durable. Nothing moves, the bar is
 * at 100 % and the rate is zero — a state that reads as "stuck" unless it is named. `finalizing`
 * begins only after that commit succeeded: host-side temporary data is being removed, so Cancel is
 * no longer meaningful and must not reclassify the durable success as an aborted transfer.
 */
export type JobPhase =
    | "idle"
    | "reading"
    | "downloading"
    | "verifying"
    | "converting"
    | "assembling"
    | "sending"
    | "committing"
    | "finalizing"
    | "done"
    | "error";

/** What a running flow is handed to report itself. */
export interface JobContext {
    /** Fires on cancel. Pass it to every await that can block, or Cancel is a lie. */
    readonly signal: AbortSignal;
    /** Cancel this whole job, including work owned by a component outside the
     * transport surface that created the context. */
    cancel(reason?: unknown): void;
    /** Move to a phase, optionally starting a fresh byte count for it. */
    phase(phase: JobPhase, total?: number): void;
    progress(done: number, total: number): void;
}
