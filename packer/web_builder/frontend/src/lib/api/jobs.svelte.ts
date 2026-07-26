import { API_BASE, api } from "./client";
import type { BuildRequest, BuildResult, BuildSession, BuildState } from "../platform/types";

// Coarse pipeline phases in order; a status event's `detail` indexes into this
// to derive an overall percentage (ported from the legacy app.js PHASES).
// "cropping" is gone since #920 — the packer no longer shells out to osmium to
// pre-crop each region, so nothing emits that phase any more.
const PHASES = ["downloading", "merging", "ingest", "bbox", "land", "quadtree", "serialize"];

const ACTIVE_JOB_KEY = "obcm.activeJob";

/**
 * Follows one build job over SSE with reactive state for the UI — the dev
 * host's `BuildSession`. The server's event log is append-only and replayed on
 * (re)connect, so reconnecting once after a dropped stream is always safe; a
 * normal stream close at job end must not be treated as an error.
 */
export class JobTracker implements BuildSession {
    state = $state<BuildState>("idle");
    phase = $state("");
    pct = $state(0);
    logLines = $state<string[]>([]);
    transientLine = $state<string | null>(null);
    result = $state<BuildResult | null>(null);
    error = $state<string | null>(null);

    private es: EventSource | null = null;
    private jobId: string | null = null;
    private reconnected = false;

    async start(req: BuildRequest) {
        this.reset();
        this.state = "starting";
        try {
            this.jobId = await api.startJob(req);
        } catch (e) {
            this.state = "error";
            this.error = e instanceof Error ? e.message : String(e);
            return;
        }
        sessionStorage.setItem(ACTIVE_JOB_KEY, this.jobId);
        this.state = "running";
        this.follow();
    }

    /** Re-attach to a build started before a page reload, if one is active. */
    async reattach(): Promise<boolean> {
        const id = sessionStorage.getItem(ACTIVE_JOB_KEY);
        if (!id) return false;
        try {
            const snap = await api.job(id);
            this.reset();
            this.jobId = id;
            if (snap.state === "done" && snap.download_url) {
                this.state = "done";
                this.pct = 100;
                this.phase = "done";
                this.result = { downloadUrl: snap.download_url, filename: snap.output, size: snap.size ?? 0 };
            } else if (snap.state === "error") {
                this.state = "error";
                this.error = snap.error ?? "Build failed.";
            } else {
                this.state = "running";
                this.follow(); // replays history, then follows live events
            }
            return true;
        } catch {
            sessionStorage.removeItem(ACTIVE_JOB_KEY); // job swept or unknown
            return false;
        }
    }

    private reset() {
        this.close();
        this.state = "idle";
        this.phase = "";
        this.pct = 0;
        this.logLines = [];
        this.transientLine = null;
        this.result = null;
        this.error = null;
        this.reconnected = false;
    }

    private follow() {
        if (!this.jobId) return;
        this.es = new EventSource(`${API_BASE}/jobs/${this.jobId}/events`);
        this.es.onmessage = (msg) => this.handle(JSON.parse(msg.data));
        this.es.onerror = () => {
            // The stream closing after done/error is normal; only a drop
            // mid-build needs action — one reconnect (events replay safely).
            if (this.state !== "running") return;
            this.close();
            if (!this.reconnected) {
                this.reconnected = true;
                this.logLines = [...this.logLines, "— connection lost, reconnecting —"];
                this.follow();
            } else {
                this.state = "error";
                this.error = "Connection to the build was lost.";
            }
        };
    }

    private handle(ev: Record<string, unknown>) {
        switch (ev.type) {
            case "status": {
                const detail = String(ev.detail ?? "");
                const i = PHASES.indexOf(detail);
                if (i >= 0) {
                    this.pct = Math.round(((i + 1) / (PHASES.length + 1)) * 100);
                    this.phase = detail;
                } else {
                    this.phase = `${ev.status}: ${detail}`;
                }
                break;
            }
            case "progress":
                if (ev.phase === "download") {
                    this.pct = Math.round(((ev.pct as number) / 100) * (100 / (PHASES.length + 1)));
                    this.phase = `downloading ${ev.region} ${ev.pct}%`;
                }
                break;
            case "log":
                if (ev.transient) {
                    this.transientLine = String(ev.line);
                } else {
                    this.transientLine = null;
                    this.logLines = [...this.logLines, String(ev.line)];
                }
                break;
            case "done":
                this.pct = 100;
                this.phase = "done";
                this.state = "done";
                this.result = {
                    downloadUrl: String(ev.download_url ?? ""),
                    filename: String(ev.output ?? ""),
                    size: (ev.size as number) ?? 0,
                };
                this.close();
                break;
            case "error":
                this.state = "error";
                this.error = String(ev.message ?? "Build failed.");
                this.close();
                break;
        }
    }

    private close() {
        this.es?.close();
        this.es = null;
    }
}
