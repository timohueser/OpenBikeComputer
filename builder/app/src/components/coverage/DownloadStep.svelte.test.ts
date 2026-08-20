// @vitest-environment happy-dom

import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const seams = vi.hoisted(() => ({
    sendMapBlob: vi.fn(),
    readMapOutput: vi.fn(async () => new Blob([Uint8Array.of(1, 2, 3, 4)])),
}));

vi.mock("../../lib/cells/store", () => ({
    cellStoreRevision: () => "test-revision",
    cellStoreWritable: vi.fn(async () => false),
    clearCellStores: vi.fn(async () => undefined),
    clearMapWorkStorage: vi.fn(async () => undefined),
    discardCellStore: vi.fn(async () => undefined),
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

vi.mock("../../lib/device/write", () => ({ sendMapBlob: seams.sendMapBlob }));

import { DeviceJob } from "../../lib/device/job.svelte";
import type { FlatStoreClient } from "../../lib/usb/client";
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
                        data: { type: "stored-map", sha256: "abc", byteLength: 4 },
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
    });

    afterEach(() => {
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
        await unmount(component);
    });
});
