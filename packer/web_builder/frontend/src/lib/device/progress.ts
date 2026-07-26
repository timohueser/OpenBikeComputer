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
 * `downloading` and `sending` are deliberately distinct: they run at completely different speeds
 * (a CDN fills a scratch file at network speed; the device drains it at SD-card speed), they fail
 * for different reasons, and the second is the one that takes the minutes.
 */
export type JobPhase = "idle" | "reading" | "downloading" | "verifying" | "sending" | "done" | "error";

/** What a running flow is handed to report itself. */
export interface JobContext {
    /** Fires on cancel. Pass it to every await that can block, or Cancel is a lie. */
    readonly signal: AbortSignal;
    /** Move to a phase, optionally starting a fresh byte count for it. */
    phase(phase: JobPhase, total?: number): void;
    progress(done: number, total: number): void;
}
