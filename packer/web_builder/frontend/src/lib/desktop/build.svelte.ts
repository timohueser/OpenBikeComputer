// The desktop host's `BuildSession`: a Tauri channel instead of an EventSource.
//
// The dev host and this one produce the same observable state from the same
// event vocabulary — `lib/build/phases.ts` owns the phase list and the bar
// arithmetic, so what differs here is genuinely only the transport and the two
// things a real filesystem adds:
//
//   * **A path, not a download URL.** The build writes into the user's maps
//     folder; `revealFile` opens it. Streaming a 200 MB `.obcm` back through the
//     webview so it can be "downloaded" onto the disk it is already on would be
//     theatre.
//   * **A cancel that cancels.** `build_cancel` trips a token the packer reads
//     inside its ingest and simplify loops, so the work stops rather than the UI
//     looking away.

import { Channel } from "@tauri-apps/api/core";
import { invoke } from "@tauri-apps/api/core";
import { downloadPct, phasePct } from "../build/phases";
import { desktop, type BuildEvent } from "./invoke";
import type { BuildRequest, BuildResult, BuildSession, BuildState } from "../platform/types";

export class DesktopBuild implements BuildSession {
    state = $state<BuildState>("idle");
    phase = $state("");
    pct = $state(0);
    logLines = $state<string[]>([]);
    /** Nothing the packer prints is transient — there is no `\r` progress bar in
     *  its output — but the interface has the slot and the log pane reads it. */
    transientLine = $state<string | null>(null);
    result = $state<BuildResult | null>(null);
    error = $state<string | null>(null);

    private jobId: string | null = null;

    async start(req: BuildRequest) {
        this.reset();
        this.state = "starting";
        const channel = this.channel();
        try {
            // `request` and `onEvent` are the Rust command's parameter names.
            this.jobId = await invoke<string>("build_start", { request: req, onEvent: channel });
        } catch (e) {
            this.state = "error";
            this.error = e instanceof Error ? e.message : String(e);
            return;
        }
        this.state = "running";
    }

    /**
     * A window that reloaded mid-build. The backend kept the job and its event
     * log, so re-attaching replays everything missed and then follows live —
     * the same guarantee the dev host gets from its append-only SSE log.
     */
    async reattach(): Promise<boolean> {
        let active;
        try {
            active = await desktop.buildActive();
        } catch {
            return false;
        }
        if (!active) return false;
        this.reset();
        this.jobId = active.id;
        this.state = active.state === "running" ? "running" : "idle";
        const attached = await invoke<boolean>("build_attach", {
            id: active.id,
            onEvent: this.channel(),
        });
        if (!attached) {
            this.jobId = null;
            this.state = "idle";
        }
        return attached;
    }

    /** Non-null: this host can actually stop a build. */
    cancel = async () => {
        if (!this.jobId || this.state !== "running") return;
        // Optimistic only in the label — the terminal state still comes from the
        // backend's `cancelled` event, so a build that finished a millisecond
        // before the click still reports `done`.
        this.phase = "cancelling…";
        await desktop.buildCancel(this.jobId);
    };

    private channel(): Channel<BuildEvent> {
        const channel = new Channel<BuildEvent>();
        channel.onmessage = (ev) => this.handle(ev);
        return channel;
    }

    private reset() {
        this.state = "idle";
        this.phase = "";
        this.pct = 0;
        this.logLines = [];
        this.transientLine = null;
        this.result = null;
        this.error = null;
        this.jobId = null;
    }

    private handle(ev: BuildEvent) {
        switch (ev.type) {
            case "status": {
                const pct = phasePct(ev.detail);
                if (pct !== null) {
                    this.pct = pct;
                    this.phase = ev.detail;
                } else {
                    this.phase = `${ev.status}: ${ev.detail}`;
                }
                break;
            }
            case "progress":
                if (ev.phase === "download") {
                    this.pct = downloadPct(ev.pct);
                    this.phase = `downloading ${ev.region} ${ev.pct}%`;
                }
                break;
            case "log":
                this.logLines = [...this.logLines, ev.line];
                break;
            case "done":
                this.pct = 100;
                this.phase = "done";
                this.state = "done";
                this.result = {
                    // No URL to hand out: the file is already where the user
                    // wanted it. `path` is what the UI acts on.
                    downloadUrl: "",
                    filename: ev.filename,
                    size: ev.size,
                    path: ev.path,
                };
                break;
            case "cancelled":
                this.state = "cancelled";
                this.phase = "cancelled";
                break;
            case "error":
                this.state = "error";
                this.error = ev.message;
                break;
        }
    }
}
