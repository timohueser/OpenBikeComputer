import { afterEach, describe, expect, it, vi } from "vitest";
import { PreviewController, type PreviewPhase } from "./previewController";

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((yes, no) => {
        resolve = yes;
        reject = no;
    });
    return { promise, resolve, reject };
}

afterEach(() => vi.useRealTimers());

describe("PreviewController", () => {
    it("debounces rapid edits and packs only the last config", async () => {
        vi.useFakeTimers();
        const calls: string[] = [];
        const phases: PreviewPhase<string>[] = [];
        const preview = new PreviewController(
            async (input: string) => {
                calls.push(input);
                return input.toUpperCase();
            },
            (phase) => phases.push(phase),
            100,
        );

        preview.schedule("old");
        await vi.advanceTimersByTimeAsync(50);
        preview.schedule("new");
        await vi.advanceTimersByTimeAsync(100);

        expect(calls).toEqual(["new"]);
        expect(phases.at(-1)).toEqual({ kind: "ready", value: "NEW" });
    });

    it("aborts an in-flight pack and suppresses a stale result even when it ignores abort", async () => {
        vi.useFakeTimers();
        const old = deferred<string>();
        const fresh = deferred<string>();
        const signals: AbortSignal[] = [];
        const phases: PreviewPhase<string>[] = [];
        const preview = new PreviewController(
            (input: string, signal) => {
                signals.push(signal);
                return input === "old" ? old.promise : fresh.promise;
            },
            (phase) => phases.push(phase),
            10,
        );

        preview.schedule("old");
        await vi.advanceTimersByTimeAsync(10);
        preview.schedule("new");
        expect(signals[0].aborted).toBe(true);
        await vi.advanceTimersByTimeAsync(10);
        fresh.resolve("fresh-map");
        await Promise.resolve();
        old.resolve("stale-map");
        await Promise.resolve();

        expect(phases.filter((phase) => phase.kind === "ready")).toEqual([
            { kind: "ready", value: "fresh-map" },
        ]);
    });

    it("surfaces only a current pack failure", async () => {
        vi.useFakeTimers();
        const phases: PreviewPhase<string>[] = [];
        const preview = new PreviewController<string, string>(
            async () => {
                throw new Error("packer rejected min_lod");
            },
            (phase) => phases.push(phase),
            1,
        );
        preview.schedule("bad");
        await vi.advanceTimersByTimeAsync(1);
        expect(phases.at(-1)).toEqual({ kind: "error", message: "packer rejected min_lod" });
    });

    it("cancels timers and requests on disposal", async () => {
        vi.useFakeTimers();
        const calls: string[] = [];
        const preview = new PreviewController(
            async (input: string) => {
                calls.push(input);
                return input;
            },
            () => {},
            20,
        );
        preview.schedule("never");
        preview.dispose();
        await vi.advanceTimersByTimeAsync(20);
        expect(calls).toEqual([]);
    });
});
