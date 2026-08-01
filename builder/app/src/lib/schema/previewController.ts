export type PreviewPhase<T> =
    | { kind: "idle" }
    | { kind: "waiting" }
    | { kind: "packing" }
    | { kind: "ready"; value: T }
    | { kind: "error"; message: string };

/**
 * Latest-edit-wins orchestration for the semi-live schema preview.
 *
 * Debouncing avoids launching a native pack per number-input keystroke.  A new
 * edit aborts an in-flight HTTP request, while the generation check remains the
 * correctness boundary for servers or test doubles that ignore AbortSignal.
 */
export class PreviewController<I, O> {
    private timer: ReturnType<typeof setTimeout> | undefined;
    private active: AbortController | undefined;
    private generation = 0;
    private disposed = false;

    constructor(
        private readonly run: (input: I, signal: AbortSignal) => Promise<O>,
        private readonly update: (phase: PreviewPhase<O>) => void,
        private readonly delayMs = 650,
    ) {}

    schedule(input: I) {
        if (this.disposed) return;
        const generation = ++this.generation;
        clearTimeout(this.timer);
        this.active?.abort();
        this.active = undefined;
        this.update({ kind: "waiting" });
        this.timer = setTimeout(() => void this.start(input, generation), this.delayMs);
    }

    private async start(input: I, generation: number) {
        if (this.disposed || generation !== this.generation) return;
        const controller = new AbortController();
        this.active = controller;
        this.update({ kind: "packing" });
        try {
            const value = await this.run(input, controller.signal);
            if (!this.disposed && generation === this.generation) this.update({ kind: "ready", value });
        } catch (cause) {
            if (this.disposed || generation !== this.generation || controller.signal.aborted) return;
            this.update({
                kind: "error",
                message: cause instanceof Error ? cause.message : String(cause),
            });
        } finally {
            if (this.active === controller) this.active = undefined;
        }
    }

    dispose() {
        this.disposed = true;
        ++this.generation;
        clearTimeout(this.timer);
        this.active?.abort();
        this.active = undefined;
        this.update({ kind: "idle" });
    }
}
