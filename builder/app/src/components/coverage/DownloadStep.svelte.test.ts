// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

interface TestCellStore {
    revision: string;
    alive: boolean;
    has: ReturnType<typeof vi.fn<(key: string, bytes: number) => Promise<boolean>>>;
    put: ReturnType<typeof vi.fn<(key: string, bytes: Uint8Array) => Promise<undefined>>>;
}

const seams = vi.hoisted(() => ({
    sendMapBlob: vi.fn(),
    sendMapBytes: vi.fn(),
    readMapOutput: vi.fn(async () => new Blob([Uint8Array.of(1, 2, 3, 4)])),
    cellStoreWritable: vi.fn(async () => false),
    clearAssemblyStorage: vi.fn(async () => undefined),
    clearCellStores: vi.fn(async () => undefined),
    clearMapWorkStorage: vi.fn(async () => undefined),
    hasRoomFor: vi.fn(async (_bytes: number, _reclaimable = 0) => false),
    openCellInventory: vi.fn<() => Promise<TestCellStore | null>>(async () => null),
    openCellStore: vi.fn<() => Promise<TestCellStore | null>>(async () => null),
    downloadCells: vi.fn(),
    discardCellStore: vi.fn(async () => undefined),
    discardMapOutput: vi.fn(async () => undefined),
    reclaimableAssemblyBytes: vi.fn(async () => 0),
    saveBlob: vi.fn(),
    workerOutput: "stored" as "stored" | "file",
    /** Make the worker end the run with a `done` the protocol guard rejects. */
    unreadableDone: false,
    workerError: null as string | null,
    workerAssemble: 0,
    requireDisk: false,
    memoryRequiresDisk: false,
    holdEstimates: false,
    estimateReplies: [] as Array<(error?: boolean) => void>,
    workerTerminate: 0,
    worker: null as null | { onmessage: ((event: MessageEvent) => void) | null },
    holdAssembly: false,
    plan: { items: [], totalBytes: 0, knownEmpty: [] } as {
        items: Array<{ band: string | null; cell: { id: string; sha256: string; bytes: number; partial?: boolean } }>;
        totalBytes: number;
        knownEmpty: never[];
    },
}));

vi.mock("../../lib/cells/store", () => ({
    cellStoreRevision: () => "test-revision",
    cellStoreWritable: seams.cellStoreWritable,
    clearAssemblyStorage: seams.clearAssemblyStorage,
    clearCellStores: seams.clearCellStores,
    clearMapWorkStorage: seams.clearMapWorkStorage,
    discardCellStore: seams.discardCellStore,
    discardMapOutput: seams.discardMapOutput,
    hasRoomFor: seams.hasRoomFor,
    openCellInventory: seams.openCellInventory,
    openCellStore: seams.openCellStore,
    readMapOutput: seams.readMapOutput,
    reclaimableAssemblyBytes: seams.reclaimableAssemblyBytes,
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

    constructor() {
        seams.worker = this;
    }

    postMessage(request: { type?: string; estimateId?: number; onDisk?: boolean; requireDisk?: boolean }) {
        if (request.type === "estimate") {
            const reply = (error = false) => this.onmessage?.(
                    new MessageEvent("message", {
                        data: error ? { type: "error", estimateId: request.estimateId, code: "internal", message: "stale estimate" } : {
                            type: "estimate-result",
                            estimateId: request.estimateId,
                            onDisk: request.onDisk,
                            estimate: {
                                engineBytes: 1,
                                inputBytes: 1,
                                outputBytes: 1,
                                peakBytes: 3,
                                budgetBytes: 100,
                                ceilingBytes: 100,
                                headroomBytes: 97,
                                fits: !seams.memoryRequiresDisk || request.onDisk === true,
                            },
                        },
                    }),
                );
            seams.estimateReplies.push(reply);
            if (!seams.holdEstimates) queueMicrotask(() => reply());
        } else if (request.type === "assemble") {
            seams.workerAssemble += 1;
            seams.requireDisk = request.requireDisk ?? false;
            if (seams.holdAssembly) return;
            queueMicrotask(() => {
                if (seams.workerError) {
                    this.onmessage?.(
                        new MessageEvent("message", {
                            data: { type: "error", code: "io", message: seams.workerError },
                        }),
                    );
                    return;
                }
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
                            data: seams.unreadableDone
                                ? { type: "done", warnings: [], summary: {} }
                                : { type: "done", warnings: [], summary: {}, wasmMemoryBytes: 16 * 1024 * 1024 },
                        }),
                    ),
                );
            });
        }
    }

    terminate() {
        seams.workerTerminate += 1;
    }
}

describe("direct assembler delivery", () => {
    beforeEach(() => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        vi.stubGlobal("Worker", AssembleWorker);
        try {
            globalThis.localStorage?.removeItem("obcm.keepMapCells");
        } catch {}
        seams.sendMapBlob.mockReset();
        seams.sendMapBytes.mockReset();
        seams.cellStoreWritable.mockReset().mockResolvedValue(false);
        seams.clearAssemblyStorage.mockReset().mockResolvedValue(undefined);
        seams.clearCellStores.mockReset().mockResolvedValue(undefined);
        seams.clearMapWorkStorage.mockReset().mockResolvedValue(undefined);
        seams.hasRoomFor.mockReset().mockResolvedValue(false);
        seams.openCellInventory.mockReset().mockResolvedValue(null);
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
        seams.reclaimableAssemblyBytes.mockReset().mockResolvedValue(0);
        seams.saveBlob.mockClear();
        seams.workerOutput = "stored";
        seams.unreadableDone = false;
        seams.workerError = null;
        seams.workerAssemble = 0;
        seams.requireDisk = false;
        seams.memoryRequiresDisk = false;
        seams.holdEstimates = false;
        seams.estimateReplies = [];
        seams.workerTerminate = 0;
        seams.worker = null;
        seams.holdAssembly = false;
        seams.plan = { items: [], totalBytes: 0, knownEmpty: [] };
    });

    afterEach(() => {
        deviceHolder.interrupted = null;
        vi.useRealTimers();
        vi.unstubAllGlobals();
        document.body.replaceChildren();
    });

    it("shows the projected memory refusal and closes Download map", async () => {
        seams.memoryRequiresDisk = true;
        const { component, target } = await mountReadyStep();
        const refusal = target.querySelector(".warn")?.textContent ?? "";
        expect(refusal).toContain("3 B of browser memory");
        expect(refusal).toContain("100 B");
        expect(refusal).toContain("Reduce the coverage area");
        expect((target.querySelector("button.primary") as HTMLButtonElement).disabled).toBe(true);
        await unmount(component);
    });

    it("shows reported download and assembly progress in every worker phase", async () => {
        seams.holdAssembly = true;
        let report!: (progress: { completedCells: number; totalCells: number; receivedBytes: number; totalBytes: number }) => void;
        let finishDownload!: () => void;
        seams.downloadCells.mockImplementation((_plan, options) => new Promise<void>((resolve) => {
            report = options.onProgress!;
            finishDownload = resolve;
        }));
        const { component, target } = await mountReadyStep();
        (target.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 30 && !report; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(report).toBeDefined();
        report({ completedCells: 1, totalCells: 2, receivedBytes: 500, totalBytes: 1_000 });
        await tick();
        expect(target.textContent).toContain("downloading cells — 1/2");
        expect(target.textContent).toContain("500 B of 1000 B");

        finishDownload();
        for (let attempt = 0; attempt < 30 && seams.workerAssemble === 0; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(seams.workerAssemble).toBe(1);
        for (const [phase, label, fraction] of [
            ["open", "reading cells", 0.1],
            ["poi", "merging places", 0.3],
            ["nav", "stitching the road network", 0.5],
            ["plan", "planning the file", 0.7],
            ["write", "writing", 0.8],
            ["verify", "checking the result", 0.9],
        ] as const) {
            seams.worker!.onmessage!(new MessageEvent("message", { data: { type: "progress", phase, fraction } }));
            await tick();
            expect(target.textContent).toContain(`assembling — ${label} · ${Math.round(fraction * 100)}%`);
        }
        [...target.querySelectorAll("button")].find((button) => button.textContent === "Cancel")!.click();
        await unmount(component);
    });

    it("refuses lost required cell storage before downloading", async () => {
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(true);
        const { component } = await mountReadyStep();
        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(job.error).toContain("storage required");
        expect(seams.downloadCells).not.toHaveBeenCalled();
        expect(seams.workerAssemble).toBe(0);
        await unmount(component);
    });

    it("carries disk admission after the running effect clears the estimate", async () => {
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(true);
        seams.openCellStore.mockResolvedValue(testCellStore());
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const { component } = await mountReadyStep();
        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(seams.requireDisk).toBe(true);
        expect(seams.workerAssemble).toBe(1);
        expect(seams.clearAssemblyStorage).toHaveBeenCalledOnce();
        await unmount(component);
    });

    it("admits a disk-only rebuild when cached cells make the quota projection fit", async () => {
        seams.memoryRequiresDisk = true;
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockImplementation(async (bytes: number) => bytes <= 2_200_000_000);
        const cells = testCellStore();
        cells.has.mockImplementation(async (key) => key === "cached");
        seams.openCellInventory.mockResolvedValue(cells);
        seams.openCellStore.mockResolvedValue(cells);
        seams.plan = {
            items: [
                { band: "fine", cell: { id: "cell-1", sha256: "cached", bytes: 800_000_000 } },
                { band: "fine", cell: { id: "cell-2", sha256: "missing", bytes: 200_000_000 } },
            ],
            totalBytes: 1_000_000_000,
            knownEmpty: [],
        };
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const large = {
            ...store,
            ledger: {
                totalBytes: 1_000_000_000,
                cellCount: 2,
                core: { bytes: 400_000_000 },
                terrain: null,
                isFinal: true,
            },
        };

        const { component, target } = await mountReadyStep({ store: large });
        (target.querySelector('.cell-storage input[type="checkbox"]') as HTMLInputElement).click();
        await tick();
        await waitForReady(target);
        expect(seams.hasRoomFor).toHaveBeenCalledWith(2_200_000_000, 0);

        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(seams.requireDisk).toBe(true);
        expect(seams.workerAssemble).toBe(1);
        expect(cells.has).toHaveBeenCalledWith("cached", 800_000_000);
        expect(cells.has).toHaveBeenCalledWith("missing", 200_000_000);
        await unmount(component);
    });

    it("keeps prior assembly files when the fresh disk preflight refuses", async () => {
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(false);
        seams.openCellStore.mockResolvedValue(testCellStore());
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const { component } = await mountReadyStep();
        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(seams.clearAssemblyStorage).not.toHaveBeenCalled();
        expect(seams.workerAssemble).toBe(1);
        await unmount(component);
    });

    it("ignores stale estimate success and failure while a new selection is pending", async () => {
        seams.holdEstimates = true;
        seams.cellStoreWritable.mockResolvedValue(true);
        seams.hasRoomFor.mockResolvedValue(true);
        const ledgers = new SvelteMap([["current", store.ledger]]);
        const changing = { ...store, get ledger() { return ledgers.get("current")!; } };
        const { component, target } = await mountReadyStep({ store: changing });
        const oldReply = seams.estimateReplies[0];
        seams.hasRoomFor.mockResolvedValue(false);
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        ledgers.set("current", { ...store.ledger, totalBytes: 8 });
        await expectSecondRunRefused(component);
        await tick();
        vi.advanceTimersByTime(500);
        for (let attempt = 0; attempt < 10 && seams.estimateReplies.length < 2; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(seams.estimateReplies).toHaveLength(2);
        oldReply();
        oldReply(true);
        await tick();
        expect(target.textContent).not.toContain("stale estimate");
        await expectSecondRunRefused(component);
        seams.estimateReplies[1]();
        oldReply();
        await tick();
        vi.useRealTimers();
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(seams.workerAssemble).toBe(1);
        expect(seams.requireDisk).toBe(false);
        await unmount(component);
    });

    it("does not post an old storage probe after a newer selection was estimated", async () => {
        let finishOld!: (value: boolean) => void;
        seams.cellStoreWritable.mockImplementationOnce(() => new Promise((resolve) => { finishOld = resolve; }));
        const ledgers = new SvelteMap([["current", store.ledger]]);
        const changing = { ...store, get ledger() { return ledgers.get("current")!; } };
        const { component } = await mountReadyStep({ store: changing });
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        ledgers.set("current", { ...store.ledger, totalBytes: 8 });
        await tick();
        vi.advanceTimersByTime(500);
        await Promise.resolve();
        await tick();
        expect(seams.estimateReplies).toHaveLength(1);
        finishOld(true);
        await Promise.resolve();
        await tick();
        expect(seams.estimateReplies).toHaveLength(1);
        vi.useRealTimers();
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const job = new DeviceJob("map");
        await job.run((ctx) => component.sendToDevice({} as FlatStoreClient, ctx), () => "sent");
        expect(seams.workerAssemble).toBe(1);
        expect(seams.requireDisk).toBe(false);
        await unmount(component);
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
            lightSkin: { name: "Default" },
            darkSkin: { name: "Dusk" },
            rootBody: "{}",
            holeCells: () => [],
        };
        const { component, target } = await mountReadyStep({ store });
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

    it("terminates the idle estimate worker when the step unmounts", async () => {
        const { component } = await mountReadyStep();

        expect(seams.workerAssemble).toBe(0);
        expect(seams.workerTerminate).toBe(0);
        await unmount(component);

        expect(seams.workerTerminate).toBe(1);
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

    it("finishes committed cleanup before terminating the worker after its step unmounts", async () => {
        let releaseCleanup!: () => void;
        seams.discardMapOutput.mockImplementationOnce(
            () => new Promise<undefined>((resolve) => (releaseCleanup = () => resolve(undefined))),
        );
        seams.sendMapBlob.mockResolvedValue({ objectId: 1n });
        const built = await mountReadyStep();
        const job = new DeviceJob("map");
        const running = job.run(
            (ctx) => built.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "sent",
        );
        for (let attempt = 0; attempt < 20 && !built.target.textContent?.includes("finishing up"); attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(job.phase).toBe("finalizing");

        await unmount(built.component);
        expect(job.running).toBe(true);
        expect(seams.workerTerminate).toBe(0);
        releaseCleanup();
        await running;

        expect(job.phase).toBe("done");
        expect(seams.discardMapOutput).toHaveBeenCalledOnce();
        expect(seams.workerTerminate).toBe(1);
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

    it("keeps a remount and its run behind an unmounted component's deferred cell write", async () => {
        const firstBuilt = await mountReadyStep();
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
            (ctx) => firstBuilt.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "unreachable",
        );
        for (let attempt = 0; attempt < 20 && typeof releasePut !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }

        await unmount(firstBuilt.component);
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        const secondBuilt = await mountReadyStep();
        await expectSecondRunRefused(secondBuilt.component);
        expect(first.running).toBe(true);
        expect(seams.clearMapWorkStorage).toHaveBeenCalledOnce();

        releasePut();
        await cancelled;
        expect(firstStore.alive).toBe(false);
        for (let attempt = 0; attempt < 20 && seams.clearMapWorkStorage.mock.calls.length < 2; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(seams.clearMapWorkStorage).toHaveBeenCalledTimes(2);

        const secondStore = testCellStore();
        seams.openCellStore.mockImplementation(async () => {
            current = secondStore;
            return secondStore;
        });
        seams.downloadCells.mockImplementationOnce(async (plan, options) => {
            const item = (plan as typeof seams.plan).items[0];
            await options.onCell(item, Uint8Array.of(5, 6, 7, 8));
        });
        await waitForReady(secondBuilt.target);
        let commit!: (result: { objectId: bigint }) => void;
        seams.sendMapBlob.mockImplementationOnce(() => new Promise((resolve) => (commit = resolve)));
        const second = new DeviceJob("map");
        const survived = second.run(
            (ctx) => secondBuilt.component.sendToDevice({} as FlatStoreClient, ctx),
            () => "second survived",
        );
        for (let attempt = 0; attempt < 20 && typeof commit !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }
        expect(secondStore.alive).toBe(true);
        expect(secondStore.put).toHaveBeenCalledWith("digest", Uint8Array.of(5, 6, 7, 8));
        commit({ objectId: 2n });
        await expect(survived).resolves.toMatchObject({ objectId: 2n });
        await unmount(secondBuilt.component);
    });

    it("keeps map actions disabled until destructive mount maintenance finishes", async () => {
        let releaseMountClear!: () => void;
        seams.clearMapWorkStorage.mockImplementationOnce(
            () => new Promise<undefined>((resolve) => (releaseMountClear = () => resolve(undefined))),
        );
        const target = document.createElement("div");
        document.body.append(target);
        const component = mount(DownloadStep, { target, props: { store: store as never } });
        await tick();
        vi.advanceTimersByTime(500);
        await Promise.resolve();
        await tick();
        vi.useRealTimers();
        for (let attempt = 0; attempt < 20 && typeof releaseMountClear !== "function"; attempt++) {
            await Promise.resolve();
            await tick();
        }

        const button = [...target.querySelectorAll("button")].find((candidate) => candidate.textContent === "Download map");
        expect(button).toBeDefined();
        expect((button as HTMLButtonElement).disabled).toBe(true);
        await expectSecondRunRefused(component);
        expect(seams.workerAssemble).toBe(0);

        releaseMountClear();
        await waitForReady(target);
        seams.sendMapBlob.mockResolvedValueOnce({ objectId: 3n });
        const run = new DeviceJob("map");
        await expect(
            run.run(
                (ctx) => component.sendToDevice({} as FlatStoreClient, ctx),
                () => "started after maintenance",
            ),
        ).resolves.toMatchObject({ objectId: 3n });
        expect(run.result).toBe("started after maintenance");
        await unmount(component);
    });

    /** A stray message is droppable; the run's only ending is not. Dropping a `done` the guard
     *  rejects would leave the screen waiting for a message that has already been sent. */
    it("fails the run when the finished result is one this build cannot read", async () => {
        seams.unreadableDone = true;
        const { component, target } = await mountReadyStep();

        (target.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 40 && !target.textContent?.includes("Nothing was saved"); attempt++) {
            await new Promise<void>((resolve) => setTimeout(resolve, 10));
            await tick();
        }

        expect(target.textContent).toContain("Nothing was saved");
        expect(seams.saveBlob).not.toHaveBeenCalled();
        await unmount(component);
    });

    it("tells the rider to reduce the selection only for a quota failure", async () => {
        seams.workerError =
            "The browser ran out of storage while building this map. Reduce the map selection and try again.";
        const quota = await mountReadyStep();
        (quota.target.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 40 && !quota.target.textContent?.includes("ran out of storage"); attempt++) {
            await new Promise<void>((resolve) => setTimeout(resolve, 10));
            await tick();
        }
        expect(quota.target.textContent).toContain("ran out of storage");
        expect(quota.target.textContent).toContain("Reduce the map selection");
        await unmount(quota.component);

        seams.workerError = "the scratch handle was closed";
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        const ordinary = await mountReadyStep();
        (ordinary.target.querySelector("button.primary") as HTMLButtonElement).click();
        for (let attempt = 0; attempt < 40 && !ordinary.target.textContent?.includes("scratch handle"); attempt++) {
            await new Promise<void>((resolve) => setTimeout(resolve, 10));
            await tick();
        }
        expect(ordinary.target.textContent).toContain("the scratch handle was closed");
        expect(ordinary.target.textContent).not.toContain("ran out of storage");
        expect(ordinary.target.textContent).not.toContain("Reduce the map selection");
        await unmount(ordinary.component);
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
    lightSkin: { name: "Default" },
    darkSkin: { name: "Dusk" },
    rootBody: "{}",
    holeCells: () => [],
};

async function mountReadyStep(
    props: { onSendReadyChange?: (ready: boolean) => void; store?: typeof store } = {},
) {
    const target = document.createElement("div");
    document.body.append(target);
    const component = mount(DownloadStep, { target, props: { ...props, store: (props.store ?? store) as never } });
    await tick();
    for (let attempt = 0; attempt < 10 && target.textContent?.includes("Deleting…"); attempt++) {
        await Promise.resolve();
        await tick();
    }
    vi.advanceTimersByTime(500);
    for (let attempt = 0; attempt < 10; attempt++) {
        await Promise.resolve();
        await tick();
    }
    vi.useRealTimers();
    return { component, target };
}

function testCellStore(): TestCellStore {
    return {
        revision: "test-revision",
        alive: true,
        has: vi.fn(async (_key: string, _bytes: number) => false),
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
