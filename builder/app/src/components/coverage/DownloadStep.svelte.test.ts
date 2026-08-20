// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const seams = vi.hoisted(() => ({
    sendMapBlob: vi.fn(),
    sendMapBytes: vi.fn(),
    readMapOutput: vi.fn(async () => new Blob([Uint8Array.of(1, 2, 3, 4)])),
    discardCellStore: vi.fn(async () => undefined),
    discardMapOutput: vi.fn(async () => undefined),
    saveBlob: vi.fn(),
    workerOutput: "stored" as "stored" | "file",
}));

vi.mock("../../lib/cells/store", () => ({
    cellStoreRevision: () => "test-revision",
    cellStoreWritable: vi.fn(async () => false),
    clearCellStores: vi.fn(async () => undefined),
    clearMapWorkStorage: vi.fn(async () => undefined),
    discardCellStore: seams.discardCellStore,
    discardMapOutput: seams.discardMapOutput,
    hasRoomFor: vi.fn(async () => false),
    openCellStore: vi.fn(),
    readMapOutput: seams.readMapOutput,
}));

vi.mock("../../lib/catalog/download", () => ({
    planCells: () => ({ items: [], totalBytes: 0, knownEmpty: [] }),
    downloadCells: vi.fn(
        async (
            _plan: unknown,
            options: { onProgress?: (progress: Record<string, number>) => void },
        ) => {
            options.onProgress?.({
                completedCells: 0,
                totalCells: 0,
                receivedBytes: 0,
                totalBytes: 0,
            });
        },
    ),
}));

vi.mock("../../lib/device/write", () => ({
    sendMapBlob: seams.sendMapBlob,
    sendMapBytes: seams.sendMapBytes,
}));
vi.mock("../../lib/download", () => ({ saveBlob: seams.saveBlob }));

import { DeviceJob } from "../../lib/device/job.svelte";
import { deviceHolder } from "../../lib/device/session.svelte";
import { DeviceError, type FlatStoreClient } from "../../lib/usb/client";
import DownloadStep from "./DownloadStep.svelte";

class AssembleWorker {
    onmessage: ((event: MessageEvent) => void) | null = null;
    onerror: ((event: ErrorEvent) => void) | null = null;
    onmessageerror: (() => void) | null = null;

    postMessage(request: { type?: string }) {
        if (request.type === "estimate") {
            queueMicrotask(() =>
                this.onmessage?.(
                    new MessageEvent("message", {
                        data: {
                            type: "estimate-result",
                            estimate: {
                                engineBytes: 1,
                                inputBytes: 1,
                                outputBytes: 1,
                                peakBytes: 3,
                                budgetBytes: 100,
                                ceilingBytes: 100,
                                headroomBytes: 97,
                                fits: true,
                            },
                        },
                    }),
                ),
            );
        } else if (request.type === "assemble") {
            queueMicrotask(() => {
                this.onmessage?.(
                    new MessageEvent("message", {
                        data: seams.workerOutput === "stored"
                            ? { type: "stored-map", sha256: "abc", byteLength: 4 }
                            : {
                                type: "file",
                                sha256: "abc",
                                byteLength: 4,
                                bytes: Uint8Array.of(1, 2, 3, 4),
                            },
                    }),
                );
                queueMicrotask(() =>
                    this.onmessage?.(
                        new MessageEvent("message", {
                            data: { type: "done", warnings: [], summary: {} },
                        }),
                    ),
                );
            });
        }
    }

    terminate() {}
}

describe("direct assembler delivery", () => {
    beforeEach(() => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        vi.stubGlobal("Worker", AssembleWorker);
        seams.sendMapBlob.mockReset();
        seams.sendMapBytes.mockReset();
        seams.discardCellStore.mockReset().mockResolvedValue(undefined);
        seams.discardMapOutput.mockClear();
        seams.saveBlob.mockClear();
        seams.workerOutput = "stored";
    });

    afterEach(() => {
        deviceHolder.interrupted = null;
        vi.useRealTimers();
        vi.unstubAllGlobals();
        document.body.replaceChildren();
    });

    it("makes the Step 3 Cancel abort PUT before it can commit", async () => {
        let committed = false;
        let putAborted = false;
        seams.sendMapBlob.mockImplementation(
            (_client: unknown, _blob: Blob, _name: string, ctx: { signal: AbortSignal }) =>
                new Promise((resolve, reject) => {
                    if (ctx.signal.aborted) {
                        putAborted = true;
                        reject(ctx.signal.reason);
                        return;
                    }
                    const commit = setTimeout(() => {
                        committed = true;
                        resolve({ objectId: 1n });
                    }, 100);
                    ctx.signal.addEventListener(
                        "abort",
                        () => {
                            putAborted = true;
                            clearTimeout(commit);
                            reject(ctx.signal.reason);
                        },
                        { once: true },
                    );
                }),
        );
        const ledger = {
            totalBytes: 4,
            cellCount: 1,
            core: { bytes: 4 },
            terrain: null,
            isFinal: true,
            verdict: { kind: "ok" },
        };
        const store = {
            ledger,
            resolution: { cellsByBand: new Map(), parts: [] },
            indices: new Map(),
            catalog: {
                schema: {
                    name: "Test schema",
                    bands: [{ id: "fine", lods: [16], role: "fine", cell_log2: 18 }],
                },
            },
            terrain: null,
            client: { fetchImpl: globalThis.fetch },
            selection: { parts: [] },
            skin: { name: "Default" },
            rootBody: "{}",
            holeCells: () => [],
        };
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(DownloadStep, {
            target,
            props: { store: store as never },
        });

        // The mandatory memory preflight is deliberately debounced by 500 ms.
        await tick();
        vi.advanceTimersByTime(500);
        await Promise.resolve();
        await tick();
        // Only the preflight debounce needs a clock. Restore the real event
        // model before exercising AbortSignal and the component click.
        vi.useRealTimers();
        const job = new DeviceJob("map");
        const running = job.run(
            (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );
        for (let attempt = 0; attempt < 20 && !target.textContent?.includes("sending the map"); attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(target.textContent).toContain("sending the map");

        const cancel = [...target.querySelectorAll("button")].find((button) => button.textContent === "Cancel");
        expect(cancel).toBeDefined();
        (cancel as HTMLButtonElement).click();
        await running;

        expect(putAborted).toBe(true);
        expect(committed).toBe(false);
        expect(job.phase).toBe("idle");
        expect(seams.discardMapOutput).toHaveBeenCalledOnce();
        await unmount(component);
    });

    it("sends a resident fallback without a duplicate Blob and removes direct staging", async () => {
        seams.workerOutput = "file";
        seams.sendMapBytes.mockResolvedValue({ objectId: 1n });
        const { component } = await mountReadyStep();
        const job = new DeviceJob("map");

        const result = await job.run(
            (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );

        expect(result).toEqual({ objectId: 1n });
        expect(seams.sendMapBytes).toHaveBeenCalledOnce();
        expect(seams.sendMapBytes.mock.calls[0][1]).toEqual(Uint8Array.of(1, 2, 3, 4));
        expect(seams.sendMapBlob).not.toHaveBeenCalled();
        expect(seams.discardMapOutput).toHaveBeenCalledOnce();
        await unmount(component);
    });

    it("preserves a physical link failure when teardown cancels direct delivery", async () => {
        let rejectPut: ((cause: unknown) => void) | null = null;
        seams.sendMapBlob.mockImplementation(
            () => new Promise((_resolve, reject) => (rejectPut = reject)),
        );
        const { component } = await mountReadyStep();
        const job = new DeviceJob("map");
        const running = job.run(
            (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );
        for (let attempt = 0; attempt < 20 && rejectPut === null; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(rejectPut).not.toBeNull();

        rejectPut!(new DeviceError("link", "the USB cable disconnected"));
        await unmount(component);
        await running;

        expect(deviceHolder.interrupted).toContain("plug it back in");
        expect(seams.discardMapOutput).toHaveBeenCalledOnce();
    });

    it("keeps every delivery action closed until deferred cleanup has finished", async () => {
        let releaseCleanup!: () => void;
        const cleanup = new Promise<undefined>((resolve) => (releaseCleanup = () => resolve(undefined)));
        seams.discardMapOutput.mockImplementationOnce(() => cleanup);
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const readyChanges: boolean[] = [];
        const { component, target } = await mountReadyStep({
            onSendReadyChange: (ready) => readyChanges.push(ready),
        });
        const first = new DeviceJob("map");
        const running = first.run(
            (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );

        for (let attempt = 0; attempt < 20 && !target.textContent?.includes("finishing up"); attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(target.textContent).toContain("finishing up");
        expect(target.textContent).not.toContain("Download map");
        expect(target.textContent).not.toContain("Cancel");
        expect(readyChanges.at(-1)).toBe(false);

        const second = new DeviceJob("map");
        await expect(
            second.run(
                (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
                () => "sent twice",
            ),
        ).resolves.toBeNull();
        expect(second.error).toContain("not ready");
        expect(seams.sendMapBlob).toHaveBeenCalledOnce();

        releaseCleanup();
        await running;
        await tick();
        expect(first.phase).toBe("done");
        expect(target.textContent).toContain("Assembled and sent");
        await unmount(component);
    });

    it("keeps the ordinary download path and does not delete its source early", async () => {
        seams.workerOutput = "file";
        const { component, target } = await mountReadyStep();

        (target.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 20 && !target.textContent?.includes("Map ready"); attempt++) {
            await Promise.resolve();
            await tick();
        }

        expect(seams.saveBlob).toHaveBeenCalledOnce();
        const [blob, name] = seams.saveBlob.mock.calls[0] as [Blob, string];
        expect(blob.size).toBe(4);
        expect(name).toBe("OBC map.obcm");
        expect(seams.discardMapOutput).not.toHaveBeenCalled();
        await unmount(component);
    });
});

const ledger = {
    totalBytes: 4,
    cellCount: 1,
    core: { bytes: 4 },
    terrain: null,
    isFinal: true,
    verdict: { kind: "ok" },
};
const store = {
    ledger,
    resolution: { cellsByBand: new Map(), parts: [] },
    indices: new Map(),
    catalog: {
        schema: {
            name: "Test schema",
            bands: [{ id: "fine", lods: [16], role: "fine", cell_log2: 18 }],
        },
    },
    terrain: null,
    client: { fetchImpl: globalThis.fetch },
    selection: { parts: [] },
    skin: { name: "Default" },
    rootBody: "{}",
    holeCells: () => [],
};

async function mountReadyStep(
    props: { onSendReadyChange?: (ready: boolean) => void } = {},
) {
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(DownloadStep, { target, props: { store: store as never, ...props } });
    await tick();
    vi.advanceTimersByTime(500);
    await Promise.resolve();
    await tick();
    vi.useRealTimers();
    return { component, target };
}
