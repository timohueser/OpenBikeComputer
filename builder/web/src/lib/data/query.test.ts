import { afterEach, expect, it, vi } from "vitest";
import { CliQuery } from "./query.svelte";

afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
});

it("a poll slower than its interval still lands, and only one request is in flight", async () => {
    vi.useFakeTimers();
    const answers: ((response: Response) => void)[] = [];
    const fetch = vi.fn(() => new Promise<Response>((resolve) => answers.push(resolve)));
    vi.stubGlobal("fetch", fetch);
    const query = new CliQuery<{ n: number }>([]);
    const stop = query.poll(1_000);
    try {
        await vi.advanceTimersByTimeAsync(5_000);
        expect(fetch).toHaveBeenCalledTimes(1);

        answers[0](new Response(JSON.stringify({ exit: 0, output: { n: 1 } })));
        await vi.advanceTimersByTimeAsync(0);
        expect(query.output).toEqual({ n: 1 });
        expect(fetch).toHaveBeenCalledTimes(1);

        await vi.advanceTimersByTimeAsync(1_000);
        expect(fetch).toHaveBeenCalledTimes(2);
    } finally {
        stop();
    }
});
