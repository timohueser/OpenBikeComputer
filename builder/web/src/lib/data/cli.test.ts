import { afterEach, expect, it, vi } from "vitest";
import { runCli } from "./cli";

afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

it("a request with no answer ends after 2 min with an error and its fix", async () => {
    vi.useFakeTimers();
    // Fake timers do not drive Node's own AbortSignal.timeout, so it gets one that they drive.
    vi.spyOn(AbortSignal, "timeout").mockImplementation((ms) => {
        const controller = new AbortController();
        setTimeout(() => controller.abort(new DOMException("timed out", "TimeoutError")), ms);
        return controller.signal;
    });
    vi.stubGlobal("fetch", (_: string, init: RequestInit) => new Promise((_, reject) => init.signal?.addEventListener("abort", () => reject(init.signal?.reason))));
    let answer: Awaited<ReturnType<typeof runCli>> | undefined;
    void runCli(["setup"]).then((result) => (answer = result));

    await vi.advanceTimersByTimeAsync(119_000);
    expect(answer).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(answer).toEqual({
        ok: false,
        error: {
            code: "failed",
            message: "`obc data ui` gave no answer within 2 min",
            fix: "Start `obc data ui` on this machine again, then reload the page.",
        },
    });
});
