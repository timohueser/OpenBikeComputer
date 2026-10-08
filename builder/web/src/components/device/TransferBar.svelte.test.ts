// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeviceJob } from "../../lib/device/job.svelte";
import type { JobContext } from "../../lib/device/progress";
import TransferBar from "./TransferBar.svelte";

afterEach(() => {
    vi.useRealTimers();
    document.body.replaceChildren();
});

describe("TransferBar", () => {
    it("shows transferred bytes, total bytes, and a measured upload rate", async () => {
        vi.useFakeTimers({ toFake: ["Date"] });
        const target = document.createElement("div");
        document.body.append(target);
        const job = new DeviceJob("map");
        let context!: JobContext;
        let finish!: () => void;
        const running = job.run(async (ctx) => {
            context = ctx;
            ctx.phase("sending", 2_000);
            return new Promise<void>((resolve) => { finish = resolve; });
        }, () => "sent");
        const component = mount(TransferBar, { target, props: { job } });
        context.progress(0, 2_000);
        vi.setSystemTime(Date.now() + 1_500);
        context.progress(1_500, 2_000);
        await tick();

        expect(target.querySelector(".line")?.textContent).toContain("1.5 KB of 2.0 KB");
        expect(target.querySelector(".line")?.textContent).toContain("1000 B/s");
        expect(target.querySelector(".fill")?.getAttribute("style")).toContain("75%");

        context.phase("finalizing");
        await tick();
        expect(target.textContent).toContain("Removing temporary map data");
        expect(target.querySelector("button")).toBeNull();
        finish();
        await running;
        await unmount(component);
    });
});
