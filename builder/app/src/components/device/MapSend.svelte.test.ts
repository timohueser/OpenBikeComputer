// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import type { Ledger } from "../../lib/catalog/ledger";
import { jobRegistry } from "../../lib/device/job.svelte";
import type { JobContext } from "../../lib/device/progress";
import { deviceHolder } from "../../lib/device/session.svelte";
import type { SendAssembledMap } from "../../lib/device/write";
import { DeviceError, type FlatStoreClient } from "../../lib/usb/client";
import MapSend from "./MapSend.svelte";

const ledger = { isFinal: true } as Ledger;
const client = {} as FlatStoreClient;

afterEach(() => {
    deviceHolder.interrupted = null;
    document.body.replaceChildren();
});

describe("MapSend lifecycle", () => {
    it("cancels the owned job when disconnect unmounts the surface", async () => {
        let context: JobContext | null = null;
        const send: SendAssembledMap = (_client, ctx) => {
            context = ctx;
            return new Promise((_resolve, reject) => {
                ctx.signal.addEventListener("abort", () => reject(ctx.signal.reason), { once: true });
            });
        };
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(MapSend, {
            target,
            props: { client, ledger, sendAssembled: send, sendReady: true },
        });

        const button = target.querySelector("button.primary") as HTMLButtonElement;
        button.click();
        await tick();
        expect(context).not.toBeNull();
        expect(jobRegistry.active?.label).toBe("map");

        await unmount(component);
        await Promise.resolve();
        await tick();

        expect(context!.signal.aborted).toBe(true);
        expect(jobRegistry.active).toBeNull();
    });

    it("keeps direct send disabled until the assembler's complete preflight is ready", async () => {
        const target = document.createElement("div");
        document.body.append(target);
        const send: SendAssembledMap = async () => {
            throw new Error("must not run");
        };
        const component = mount(MapSend, {
            target,
            props: { client, ledger, sendAssembled: send, sendReady: false },
        });
        await tick();

        expect((target.querySelector("button.primary") as HTMLButtonElement).disabled).toBe(true);
        await unmount(component);
    });

    it("keeps a physical link failure when disconnect unmount also cancels the job", async () => {
        let rejectSend: ((cause: unknown) => void) | null = null;
        const send: SendAssembledMap = () =>
            new Promise((_resolve, reject) => {
                rejectSend = reject;
            });
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(MapSend, {
            target,
            props: { client, ledger, sendAssembled: send, sendReady: true },
        });
        (target.querySelector("button.primary") as HTMLButtonElement).click();
        await tick();

        rejectSend!(new DeviceError("link", "the USB cable disconnected"));
        await unmount(component);
        await Promise.resolve();
        await tick();

        expect(deviceHolder.interrupted).toContain("plug it back in");
        expect(jobRegistry.active).toBeNull();
    });
});
