/**
 * The job slot: what the three surfaces render from.
 *
 * Two of these exist because driving the real UI found the bugs and the suite did not. `running`
 * was a getter over a plain field, so the progress bar never appeared and the buttons never
 * disabled — everything "worked", invisibly. And a write killed by an unplug reported itself into a
 * component that unmounts the same instant, so the rider got a Connect button and no account of
 * what happened to their upload.
 */

import { describe, expect, it } from "vitest";

import { DeviceJob } from "./job.svelte";
import { deviceHolder } from "./session.svelte";
import type { JobPhase } from "./progress";

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("DeviceJob", () => {
    it("tracks phases, bytes and the finished sentence", async () => {
        const job = new DeviceJob();
        const seen: JobPhase[] = [];
        const running = job.run(async (ctx) => {
            seen.push("reading");
            ctx.phase("sending", 1000);
            ctx.progress(500, 1000);
            expect(job.running).toBe(true);
            expect(job.pct).toBe(50);
            return 7;
        }, (value) => `did ${value}`);
        expect(job.running, "the slot is held synchronously, before the first await").toBe(true);
        await running;
        expect(seen).toEqual(["reading"]);
        expect(job.phase).toBe("done");
        expect(job.result).toBe("did 7");
        expect(job.running).toBe(false);
    });

    it("refuses a second task while one is running", async () => {
        const job = new DeviceJob();
        const first = job.run(async () => {
            await settle();
            return 1;
        }, () => "first");
        expect(await job.run(async () => 2, () => "second")).toBeNull();
        await first;
    });

    it("keeps a failure's code, not only its message", async () => {
        const job = new DeviceJob();
        await job.run(async () => {
            throw Object.assign(new Error("The device's catalog is full."), { code: "storage-full" });
        }, () => "unreachable");
        expect(job.phase).toBe("error");
        expect(job.errorCode).toBe("storage-full");
        expect(job.error).toContain("catalog is full");
    });

    it("treats a cancel as a return to idle, not as an error", async () => {
        const job = new DeviceJob();
        const running = job.run(async (ctx) => {
            job.cancel();
            await settle();
            ctx.signal.throwIfAborted();
            return 1;
        }, () => "unreachable");
        expect(await running).toBeNull();
        expect(job.phase).toBe("idle");
        expect(job.error).toBeNull();
    });

    it("reports a lost link where it will still be on screen afterwards", async () => {
        deviceHolder.interrupted = null;
        const job = new DeviceJob();
        await job.run(async () => {
            throw Object.assign(new Error("The device disconnected."), { code: "link" });
        }, () => "unreachable");
        // The surface that ran this unmounts the moment the device goes; the holder is what
        // survives it, and the rider reads the sentence next to the Connect button.
        expect(deviceHolder.interrupted).toContain("plug it back in");
        deviceHolder.interrupted = null;
    });
});
