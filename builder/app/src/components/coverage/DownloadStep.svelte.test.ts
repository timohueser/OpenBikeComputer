// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

interface TestCellStore {
    revision: string;
    alive: boolean;
    has: ReturnType<typeof vi.fn<() => Promise<boolean>>>;
    put: ReturnType<typeof vi.fn<(key: string, bytes: Uint8Array) => Promise<undefined>>>;
}

const seams = vi.hoisted(() => ({
    sendMapBlob: vi.fn(),
    sendMapBytes: vi.fn(),
    readMapOutput: vi.fn(async () => new Blob([Uint8Array.of(1, 2, 3, 4)])),
    cellStoreWritable: vi.fn(async () => false),
    clearCellStores: vi.fn(async () => undefined),
    hasRoomFor: vi.fn(async () => false),
    openCellStore: vi.fn<() => Promise<TestCellStore | null>>(async () => null),
    downloadCells: vi.fn(),
    discardCellStore: vi.fn(async () => undefined),
    discardMapOutput: vi.fn(async () => undefined),
    saveBlob: vi.fn(),
    workerOutput: "stored" as "stored" | "file",
    workerAssemble: 0,
    plan: { items: [], totalBytes: 0, knownEmpty: [] } as {
        items: Array<{ band: string | null; cell: { id: string; sha256: string; bytes: number; partial?: boolean } }>;
        totalBytes: number;
        knownEmpty: never[];
    },
}));

vi.mock("../../lib/cells/store", () => ({
    cellStoreRevision: () => "test-revision",
    cellStoreWritable: seams.cellStoreWritable,
    clearCellStores: seams.clearCellStores,
    clearMapWorkStorage: vi.fn(async () => undefined),
    discardCellStore: seams.discardCellStore,
    discardMapOutput: seams.discardMapOutput,
    hasRoomFor: seams.hasRoomFor,
    openCellStore: seams.openCellStore,
    readMapOutput: seams.readMapOutput,
}));

vi.mock("../../lib/catalog/download", () => ({
    planCells: () => seams.plan,
    downloadCells: seams.downloadCells,
}));

vi.mock("../../lib/device/write", () => ({
    sendMapBlob: seams.sendMapBlob,
    sendMapBytes: seams.sendMapBytes,
}));
vi.mock("../../lib/download", () => ({ saveBlob: seams.saveBlob }));

import { DeviceJob, jobRegistry } from "../../lib/device/job.svelte";
import { deviceHolder } from "../../lib/device/session.svelte";
import type { SendAssembledMap } from "../../lib/device/write";
import { DeviceError, type FlatStoreClient } from "../../lib/usb/client";
import MapSend from "../device/MapSend.svelte";
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
            seams.workerAssemble += 1;
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
        seams.cellStoreWritable.mockReset().mockResolvedValue(false);
        seams.clearCellStores.mockReset().mockResolvedValue(undefined);
        seams.hasRoomFor.mockReset().mockResolvedValue(false);
        seams.openCellStore.mockReset().mockResolvedValue(null);
        seams.downloadCells.mockReset().mockImplementation(
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
        );
        seams.discardCellStore.mockReset().mockResolvedValue(undefined);
        seams.discardMapOutput.mockClear();
        seams.saveBlob.mockClear();
        seams.workerOutput = "stored";
        seams.workerAssemble = 0;
        seams.plan = { items: [], totalBytes: 0, knownEmpty: [] };
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

    it("keeps committed success when the real TransferBar is cancelled and unmounted during cleanup", async () => {
        let commit!: (result: { objectId: bigint }) => void;
        seams.sendMapBlob.mockImplementation(
            () => new Promise((resolve) => (commit = resolve)),
        );
        let releaseCleanup!: () => void;
        const cleanup = new Promise<undefined>((resolve) => (releaseCleanup = () => resolve(undefined)));
        seams.discardMapOutput.mockImplementationOnce(() => cleanup);
        const built = await mountReadyStep();
        const transferTarget = document.createElement("div");
        document.body.append(transferTarget);
        let success: unknown = null;
        let failure: unknown = null;
        const send: SendAssembledMap = async (client, ctx) => {
            try {
                success = await built.component.sendToDevice(client, ctx);
                return success as Awaited<ReturnType<SendAssembledMap>>;
            } catch (cause) {
                failure = cause;
                throw cause;
            }
        };
        const mapSend = mount(MapSend, {
            target: transferTarget,
            props: {
                client: {} as FlatStoreClient,
                ledger: ledger as never,
                sendAssembled: send,
                sendReady: true,
            },
        });

        (transferTarget.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 20 && typeof commit !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }
        const staleCancel = [...transferTarget.querySelectorAll("button")].find(
            (button) => button.textContent === "Cancel",
        ) as HTMLButtonElement;
        expect(staleCancel).toBeDefined();

        commit({ objectId: 1n });
        for (let attempt = 0; attempt < 20 && !transferTarget.textContent?.includes("Removing temporary"); attempt++) {
            await Promise.resolve();
            await tick();
        }
        const job = jobRegistry.active;
        expect(job?.phase).toBe("finalizing");
        expect(transferTarget.textContent).toContain("Removing temporary map data");
        expect(transferTarget.textContent).not.toContain("Cancel");

        // Exercise both stale UI cancellation and the surface's onDestroy cancellation after the
        // durable commit. Neither may reach DownloadStep's detached abort listener.
        staleCancel.click();
        await unmount(mapSend);
        expect(job?.running).toBe(true);
        releaseCleanup();
        for (let attempt = 0; attempt < 20 && job?.running; attempt++) {
            await Promise.resolve();
            await tick();
        }

        expect(success).toEqual({ objectId: 1n });
        expect(failure).toBeNull();
        expect(job?.phase).toBe("done");
        expect(seams.discardMapOutput).toHaveBeenCalledOnce();
        expect(built.target.textContent).toContain("Assembled and sent");
        expect(built.target.textContent).not.toContain("Cancelled");
        expect(built.target.textContent).not.toContain("Nothing was saved");
        await unmount(built.component);
    });

    it("keeps run 2 blocked until a cancelled destructive clear has returned", async () => {
        const built = await mountReadyStep();
        let releaseClear!: () => void;
        seams.clearCellStores.mockImplementationOnce(
            () => new Promise<undefined>((resolve) => (releaseClear = () => resolve(undefined))),
        );
        const first = new DeviceJob("map");
        const running = first.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );
        for (let attempt = 0; attempt < 20 && typeof releaseClear !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }

        first.cancel();
        await expectSecondRunRefused(built.component);
        expect(first.running).toBe(true);
        expect(built.target.textContent).not.toContain("Cancelled");
        releaseClear();
        await running;
        expect(first.phase).toBe("idle");

        await waitForReady(built.target);
        seams.sendMapBlob.mockResolvedValueOnce({ objectId: 2n });
        const second = new DeviceJob("map");
        await expect(
            second.run(
                (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
                () => "second survived",
            ),
        ).resolves.toMatchObject({ objectId: 2n });
        expect(second.result).toBe("second survived");
        await unmount(built.component);
    });

    it("keeps run 2 blocked through a cancelled store open and its stale cleanup", async () => {
        const built = await mountReadyStep();
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(true);
        let current: { alive: boolean } | null = null;
        seams.discardCellStore.mockImplementation(async () => {
            if (current) current.alive = false;
        });
        const firstStore = testCellStore();
        let releaseOpen!: () => void;
        seams.openCellStore.mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    releaseOpen = () => {
                        current = firstStore;
                        resolve(firstStore);
                    };
                }),
        );
        const first = new DeviceJob("map");
        const cancelled = first.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "unreachable",
        );
        for (let attempt = 0; attempt < 20 && typeof releaseOpen !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }

        first.cancel();
        await expectSecondRunRefused(built.component);
        expect(first.running).toBe(true);
        releaseOpen();
        await cancelled;
        expect(firstStore.alive).toBe(false);
        expect(seams.discardCellStore).toHaveBeenCalledOnce();

        const secondStore = testCellStore();
        seams.openCellStore.mockImplementation(async () => {
            current = secondStore;
            return secondStore;
        });
        await waitForReady(built.target);
        let commit!: (result: { objectId: bigint }) => void;
        seams.sendMapBlob.mockImplementationOnce(() => new Promise((resolve) => (commit = resolve)));
        const second = new DeviceJob("map");
        const survived = second.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "second survived",
        );
        for (let attempt = 0; attempt < 20 && typeof commit !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(secondStore.alive).toBe(true);
        expect(seams.discardCellStore).toHaveBeenCalledOnce();
        commit({ objectId: 2n });
        await expect(survived).resolves.toMatchObject({ objectId: 2n });
        await unmount(built.component);
    });

    it("keeps run 2 blocked until a cancelled cell write and its cleanup have settled", async () => {
        const built = await mountReadyStep();
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(true);
        seams.plan = {
            items: [{ band: "fine", cell: { id: "cell-1", sha256: "digest", bytes: 4 } }],
            totalBytes: 4,
            knownEmpty: [],
        };
        let current: ReturnType<typeof testCellStore> | null = null;
        seams.discardCellStore.mockImplementation(async () => {
            if (current) current.alive = false;
        });
        const firstStore = testCellStore();
        let releasePut!: () => void;
        firstStore.put.mockImplementationOnce(
            () => new Promise<undefined>((resolve) => (releasePut = () => resolve(undefined))),
        );
        seams.openCellStore.mockImplementation(async () => {
            current = firstStore;
            return firstStore;
        });
        seams.downloadCells.mockImplementationOnce(async (plan, options) => {
            const item = (plan as typeof seams.plan).items[0];
            await options.onCell(item, Uint8Array.of(1, 2, 3, 4));
        });
        const first = new DeviceJob("map");
        const cancelled = first.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "unreachable",
        );
        for (let attempt = 0; attempt < 20 && typeof releasePut !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }

        first.cancel();
        await expectSecondRunRefused(built.component);
        expect(first.running).toBe(true);
        releasePut();
        await cancelled;
        expect(firstStore.alive).toBe(false);
        const firstCleanupCount = seams.discardCellStore.mock.calls.length;

        const secondStore = testCellStore();
        seams.openCellStore.mockImplementation(async () => {
            current = secondStore;
            return secondStore;
        });
        seams.downloadCells.mockImplementationOnce(async (plan, options) => {
            const item = (plan as typeof seams.plan).items[0];
            await options.onCell(item, Uint8Array.of(5, 6, 7, 8));
        });
        await waitForReady(built.target);
        let commit!: (result: { objectId: bigint }) => void;
        seams.sendMapBlob.mockImplementationOnce(() => new Promise((resolve) => (commit = resolve)));
        const second = new DeviceJob("map");
        const survived = second.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "second survived",
        );
        for (let attempt = 0; attempt < 20 && typeof commit !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(secondStore.alive).toBe(true);
        expect(secondStore.put).toHaveBeenCalledWith("digest", Uint8Array.of(5, 6, 7, 8));
        expect(seams.discardCellStore).toHaveBeenCalledTimes(firstCleanupCount);
        commit({ objectId: 2n });
        await expect(survived).resolves.toMatchObject({ objectId: 2n });
        await unmount(built.component);
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

function testCellStore(): TestCellStore {
    return {
        revision: "test-revision",
        alive: true,
        has: vi.fn(async () => false),
        put: vi.fn(async (_key: string, _bytes: Uint8Array) => undefined),
    };
}

async function expectSecondRunRefused(component: { sendToDevice: SendAssembledMap }) {
    const second = new DeviceJob("map");
    await expect(
        second.run(
            (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
            () => "must stay blocked",
        ),
    ).resolves.toBeNull();
    expect(second.error).toContain("not ready");
}

async function waitForReady(target: HTMLElement) {
    for (let attempt = 0; attempt < 40; attempt++) {
        const button = [...target.querySelectorAll("button")].find((candidate) => candidate.textContent === "Download map");
        if (button && !(button as HTMLButtonElement).disabled) return;
        await new Promise<void>((resolve) => setTimeout(resolve, 20));
        await tick();
    }
    throw new Error("the map builder did not become ready for the next run");
}
