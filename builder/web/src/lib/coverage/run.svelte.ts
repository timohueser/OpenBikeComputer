// One map run: cells fetched and verified against the catalog, assembled into ONE `.obcm` by
// wasm in a Worker, and either downloaded normally or streamed straight to a connected device.
//
// A map is one file, so there is nothing to order, nothing to package and nothing to
// acknowledge: the only question left is where it lands.

import { ASSEMBLE_PHASES, type AssemblePhase, type MemoryEstimate } from "../assemble/bridge";
import {
    isWorkerResponse,
    requestTransferList,
    type AssembleWorkerRequest,
    type CellReadMode,
    type MapWriteMode,
    type WorkerCell,
    type WorkerSourceCell,
    type WorkerTerrainCell,
} from "../assemble/workerProtocol";
import {
    cellStoreRevision,
    cellStoreWritable,
    clearAssemblyStorage,
    clearCellStores,
    clearMapWorkStorage,
    discardCellStore,
    discardMapOutput,
    hasRoomFor,
    isStorageQuotaError,
    openCellInventory,
    openCellStore,
    readMapOutput,
    reclaimableAssemblyBytes,
    STORAGE_QUOTA_MESSAGE,
    type CellInventory,
} from "../cells/store";
import { projectedRunDiskBytes } from "../cells/quota";
import { acquireMapWorkStorage } from "../cells/workBarrier";
import { saveBlob } from "../download";
import {
    downloadCells,
    planCells,
    type CellDownloadPlan,
    type CellDownloadProgress,
} from "../catalog/download";
import type { CoverageStore } from "./store.svelte";
import type { JobContext } from "../device/progress";
import type { SendAssembledMap } from "../device/write";
import { formatBytes, truncateUtf8 } from "../format";
import type { FlatStoreClient } from "../usb/client";
import type { PutResponse } from "../usb/protocol";

const KEEP_CELLS_KEY = "obcm.keepMapCells";

function loadKeepCells(): boolean {
    try {
        return globalThis.localStorage?.getItem(KEEP_CELLS_KEY) === "true";
    } catch {
        return false;
    }
}

/** The engine's sort budget, which after the external merge **is** the wasm engine term. A
 *  chosen value: big enough that a DACH-scale sort is about a dozen runs, small enough that
 *  engine plus caches plus terrain stays a fraction of a tab's budget. Passed to both the
 *  assembly and the estimate, so the projection prices the run that will actually happen. */
const SORT_BUDGET_BYTES = 256 * 1024 * 1024;

/** A phone's tab gets nowhere near a desktop's 3 GiB before the browser
 *  evicts the page — and eviction loses the download with no error to
 *  show. 1 GiB is the bridge test's own phone-shaped judgement. */
const isMobileUa = typeof navigator !== "undefined" && /Android|iPhone|iPad|Mobile/i.test(navigator.userAgent);
const MOBILE_BUDGET = 1024 ** 3;

/** Milliseconds from each phase's start to the next phase's start, in phase order. */
function phaseDurations(starts: Partial<Record<AssemblePhase, number>>): Partial<Record<AssemblePhase, number>> {
    const seen = ASSEMBLE_PHASES.filter((p) => starts[p] !== undefined);
    const out: Partial<Record<AssemblePhase, number>> = {};
    for (let i = 0; i + 1 < seen.length; i++) {
        out[seen[i]] = Math.round(starts[seen[i + 1]]! - starts[seen[i]]!);
    }
    return out;
}

export type Phase =
    | "idle"
    | "downloading"
    | "assembling"
    | "saving"
    | "finalizing"
    | "done"
    | "cancelled"
    | "error";

interface DeviceOutput {
    readonly kind: "device";
    readonly client: FlatStoreClient;
    readonly ctx: JobContext;
    readonly resolve: (result: PutResponse) => void;
    readonly reject: (cause: unknown) => void;
    removeAbort: () => void;
    result: PutResponse | null;
    settled: boolean;
    failing: boolean;
    runId: number | null;
    transientCellRevision: string | null;
    releaseWork: (() => void) | null;
    readonly operations: Set<Promise<unknown>>;
}
interface DownloadOutput {
    readonly kind: "download";
    runId: number | null;
    transientCellRevision: string | null;
    releaseWork: (() => void) | null;
    readonly operations: Set<Promise<unknown>>;
}
type Output = DownloadOutput | DeviceOutput;

/**
 * The same plan minus every cell already in the store, and the tally of what that saved.
 *
 * A cell is "already here" when a file named by its catalog digest exists at the catalog's
 * length. The name **is** the SHA-256 `fetchVerified` matched before the file was written, and
 * the length catches a write torn by a crash or a quota refusal. Re-hashing every cell on every
 * run would cost seconds and a full read of exactly the bytes this exists to stop reading.
 *
 * Terrain is never skipped: it is not in the store.
 */
interface CachedPlan {
    plan: CellDownloadPlan;
    cells: number;
    bytes: number;
}

async function scanCached(
    plan: CellDownloadPlan,
    cells: CellInventory,
    runOp: <T>(operation: () => Promise<T>) => Promise<T> = (operation) => operation(),
): Promise<CachedPlan> {
    const wanted: typeof plan.items = [];
    let bytes = 0;
    let have = 0;
    let haveBytes = 0;
    for (const item of plan.items) {
        const present = item.band !== null && (await runOp(() => cells.has(item.cell.sha256, item.cell.bytes)));
        if (present) {
            have += 1;
            haveBytes += item.cell.bytes;
            continue;
        }
        wanted.push(item);
        bytes += item.cell.bytes;
    }
    return {
        plan: { ...plan, items: wanted, totalBytes: bytes },
        cells: have,
        bytes: haveBytes,
    };
}

/** The build of one map from a coverage store. Call `start()` once, and `destroy()` when the
 *  owner goes away. */
export class MapRun {
    readonly #store: CoverageStore;
    readonly #ledger = $derived.by(() => this.#store.ledger);

    #worker: Worker | null = null;
    #destroyed = false;

    #phase = $state<Phase>("idle");
    #dlProgress = $state<CellDownloadProgress | null>(null);
    #asmPhase = $state<AssemblePhase>("open");
    #asmFraction = $state(0);
    /** When each assembly phase began, for the run's debug ledger. */
    #asmPhaseStart: Partial<Record<AssemblePhase, number>> = {};
    /** The map, once there is one: what it is called and how big it is. */
    #savedFile = $state<{ name: string; byteLength: number } | null>(null);
    /** Whether the file actually reached the disk. Not `savedFile !== null`: it is
     *  set the moment the map is delivered, and a cancel that raced the delivery
     *  must not tell someone to go and discard a file nobody has. */
    #persisted = $state(false);
    /** The one delivery, so `done` can wait for a save/send that is still running rather
     *  than declare the map finished behind it. */
    #delivery: Promise<void> = Promise.resolve();
    /** Set by `failRun`: a delivery that has not started must not write into an
     *  output that is being discarded. */
    #sinkClosed = false;
    /** How this run's cells reached the assembler, as the worker reports it before the assembly
     *  starts. Shown, because a bug report about a failed country-scale run has to say which. */
    #readMode = $state<CellReadMode | null>(null);
    /** …and where its map went, for the same reason. */
    #writeMode = $state<MapWriteMode | null>(null);
    /** Cells this run did not have to fetch, because a previous one already put
     *  them in OPFS under the same digest. */
    #cachedCells = $state(0);
    #cachedBytes = $state(0);
    /** Opt-in reuse. OPFS may still be the active run's necessary working disk when false. */
    #keepCells = $state(loadKeepCells());
    // Starts true so the first readiness cannot briefly enable a run before
    // `start()` maintenance has joined the origin-global map-work barrier.
    #clearingCells = $state(true);
    #cellStorageNotice = $state<string | null>(null);
    #runWarnings = $state<string[]>([]);
    #errorMessage = $state<string | null>(null);
    #abortCtl: AbortController | null = null;
    /** The name this run's file will be saved under, fixed when the run starts, so editing the
     *  selection mid-download cannot rename the map that is already being built from the old one. */
    #runFileName = $state("OBC map.obcm");

    #output = $state.raw<Output | null>(null);
    #lastRunKind = $state<Output["kind"]>("download");
    #nextRunId = 0;
    #activeRunId = 0;
    #failure: Promise<void> = Promise.resolve();
    #paused = $state(false);

    #estimate = $state<MemoryEstimate | null>(null);
    #estimateLedger = $state.raw<CoverageStore["ledger"]>(null);
    #estimateOnDisk = false;
    #estimateGeneration = 0;
    #estimatePending = $state(false);
    /** The estimate's own failure channel, never mixed into the run's. Cleared by the next
     *  request; retried by bumping the nonce. */
    #estimateError = $state<string | null>(null);
    #estimateNonce = $state(0);
    readonly #disposeEstimate: () => void;

    constructor(store: CoverageStore) {
        this.#store = store;
        this.#disposeEstimate = $effect.root(() => {
            $effect(() => this.#estimateEffect());
        });
    }

    get phase(): Phase { return this.#phase; }
    get dlProgress(): CellDownloadProgress | null { return this.#dlProgress; }
    get asmPhase(): AssemblePhase { return this.#asmPhase; }
    get asmFraction(): number { return this.#asmFraction; }
    get savedFile(): { name: string; byteLength: number } | null { return this.#savedFile; }
    get persisted(): boolean { return this.#persisted; }
    get readMode(): CellReadMode | null { return this.#readMode; }
    get writeMode(): MapWriteMode | null { return this.#writeMode; }
    get cachedCells(): number { return this.#cachedCells; }
    get cachedBytes(): number { return this.#cachedBytes; }
    get keepCells(): boolean { return this.#keepCells; }
    get clearingCells(): boolean { return this.#clearingCells; }
    get cellStorageNotice(): string | null { return this.#cellStorageNotice; }
    get runWarnings(): string[] { return this.#runWarnings; }
    get errorMessage(): string | null { return this.#errorMessage; }
    get runFileName(): string { return this.#runFileName; }
    get lastRunKind(): Output["kind"] { return this.#lastRunKind; }
    get sendingToDevice(): boolean { return this.#output?.kind === "device"; }
    get estimateError(): string | null { return this.#estimateError; }
    get refreshBlocked(): boolean { return this.#refreshBlocked; }
    get memoryCaution(): string | null { return this.#memoryCaution; }
    get running(): boolean { return this.#running; }
    get refusal(): string | null { return this.#refusal; }
    get ready(): boolean { return this.#ready; }

    #ensureWorker(): Worker {
        if (!this.#worker) {
            this.#worker = new Worker(new URL("../assemble/assemble.worker.ts", import.meta.url), {
                type: "module",
            });
            const current = this.#worker;
            this.#worker.onmessage = (event) => {
                if (this.#worker === current) void this.#onWorkerMessage(event);
            };
            // The worker's own code answers every failure with an `error` message;
            // these two fire for what that code never sees — the script failing to
            // boot at all, and a message that could not be deserialized. Both arrive
            // as the protocol's `{code: "internal"}` shape, and the worker is dropped,
            // because one that could not boot stays broken.
            this.#worker.onerror = (e) => {
                if (this.#worker !== current) return;
                this.#workerFailed(e.message || "the assembly worker failed to start");
            };
            this.#worker.onmessageerror = () => {
                if (this.#worker !== current) return;
                this.#workerFailed("a message to the assembly worker could not be delivered");
            };
        }
        return this.#worker;
    }

    /** A worker-level failure, routed like a posted `error`. */
    #workerFailed(message: string) {
        this.#worker?.terminate();
        this.#worker = null;
        void this.#onWorkerMessage(
            new MessageEvent("message", { data: { type: "error", code: "internal", message } }),
        );
    }

    destroy(): void {
        this.#destroyed = true;
        const cause = new DOMException("the map builder was closed", "AbortError");
        // Finalization starts only after the map is durable and its abort listener has been
        // detached. Let that owner finish its cleanup. Every earlier teardown goes through the
        // same barrier-owning failure path; independent close/discard calls here could race the
        // owner's outstanding put or a newly started run.
        if (this.#phase !== "finalizing") {
            // Direct delivery is cancelled through the DeviceJob so the same signal reaches the
            // in-flight PUT; its one abort listener enters failRun. Browser delivery has
            // no outer controller, so it enters that owner path directly.
            if (this.#output?.kind === "device" && !this.#output.settled) this.#output.ctx.cancel(cause);
            else void this.#failRun(cause);
        }
        // With no run owner, the worker only serves the background estimate and can be dropped
        // immediately. A finalizing owner still needs it to finish its `done` continuation, so
        // that path terminates below only after cleanup and settlement.
        if (!this.#output) {
            this.#worker?.terminate();
            this.#worker = null;
        }
        this.#disposeEstimate();
    }

    start(): void {
        void this.#initializeStorage();
    }

    async #onWorkerMessage(e: MessageEvent) {
        const msg = e.data as unknown;
        if (!isWorkerResponse(msg)) {
            // A message this protocol does not speak is droppable. A malformed `done` is not:
            // it is the run's only ending, so dropping it leaves the screen waiting for a
            // result that will never come again.
            if (typeof msg === "object" && msg !== null && (msg as { type?: unknown }).type === "done") {
                await this.#failRun(new Error("The assembly finished with a result this build cannot read."));
            }
            return;
        }
        switch (msg.type) {
            case "estimate-result":
                if (msg.estimateId !== this.#estimateGeneration || this.#estimateLedger !== this.#ledger) return;
                this.#estimateOnDisk = msg.onDisk;
                this.#estimate = msg.estimate;
                this.#estimatePending = false;
                this.#estimateError = null;
                break;
            case "progress":
                this.#asmPhaseStart[msg.phase] ??= performance.now();
                this.#asmPhase = msg.phase;
                this.#asmFraction = msg.fraction;
                break;
            case "reading":
                this.#readMode = msg.mode;
                break;
            case "writing":
                this.#writeMode = msg.mode;
                break;
            case "stored-map":
                // The map never crossed the port: it is in OPFS, the worker has
                // let go of it, and this is where it becomes a file someone has.
                // The phase moves before the delivery is assigned, because a
                // ~9 GiB write outlives this handler and Cancel must dispatch on
                // the right phase.
                this.#phase = "saving";
                this.#delivery = this.#deliverStoredMap(msg.byteLength);
                void this.#delivery.catch(() => {});
                break;
            case "file":
                // No sink was available, so the bytes came across instead. Same
                // delivery, but direct device delivery keeps the worker's transferred
                // buffer as its sole resident copy instead of snapshotting it into a Blob.
                this.#phase = "saving";
                this.#delivery = this.#output?.kind === "device"
                    ? this.#deliverResidentMap(msg.bytes, msg.byteLength)
                    : this.#deliverMap(new Blob([msg.bytes as unknown as BlobPart]), msg.byteLength);
                void this.#delivery.catch(() => {});
                break;
            case "done":
                // The run's ledger: OPFS traffic, the time each phase took and the linear
                // memory the assembly ended on. For anyone profiling an assembly from
                // DevTools — a worker's own console does not surface — and the only place
                // these numbers are readable from outside the worker. Nothing on screen
                // reads them.
                console.debug("[assemble] run", {
                    wasmMemoryBytes: msg.wasmMemoryBytes,
                    io: msg.io,
                    phaseMs: phaseDurations(this.#asmPhaseStart),
                });
                this.#runWarnings = msg.warnings;
                try {
                    await this.#delivery;
                    // A direct PUT is durable now. Detach its abort listener synchronously:
                    // cancellation after commit must not race `failRun` against host-side
                    // cleanup and turn success into "cancelled".
                    if (!this.#sinkClosed && this.#output?.kind === "device") {
                        this.#output.removeAbort();
                        this.#output.ctx.phase("finalizing");
                        this.#phase = "finalizing";
                    }
                } catch (cause) {
                    await this.#failRun(cause);
                    break;
                }
                // A cancel that arrived *during* the save has already discarded the
                // output and set the phase — `failRun` awaits this same delivery, so
                // both paths resume here. Without this guard a discarded map could
                // report "done". `sinkClosed` settles it, because it is set before
                // the discard rather than after it.
                if (this.#sinkClosed) break;
                // Cleanup can recursively remove a country-sized temporary cell store. Keep the
                // run closed while that awaits: declaring `done` here briefly re-enabled both
                // delivery actions, and the older continuation could then clear a newer run's
                // shared `output` after its cleanup finished.
                this.#phase = "finalizing";
                await this.#cleanupTransientCells();
                if (this.#output?.kind === "device") {
                    if (!this.#output.result) {
                        await this.#failRun(new Error("The device did not commit the assembled map."));
                        break;
                    }
                    await this.#cleanupDirectOutput();
                    this.#settleDevice(this.#output.result);
                }
                this.#output?.releaseWork?.();
                if (this.#output) this.#output.releaseWork = null;
                this.#output = null;
                this.#abortCtl = null;
                this.#activeRunId = 0;
                this.#phase = "done";
                if (this.#destroyed) {
                    this.#worker?.terminate();
                    this.#worker = null;
                }
                break;
            case "error":
                if (
                    msg.estimateId !== undefined &&
                    (msg.estimateId !== this.#estimateGeneration || this.#estimateLedger !== this.#ledger)
                ) return;
                // Two conversations share this worker, and their failures are
                // different facts: an error during a run belongs to the run, but an
                // error answering the background *estimate* must not paint the screen
                // with "Nothing was saved" about a download that never started.
                if (this.#phase === "assembling" || this.#phase === "saving") {
                    await this.#failRun(new Error(msg.message));
                } else {
                    this.#estimateError = msg.message;
                }
                this.#estimatePending = false;
                break;
        }
    }

    /**
     * The map the assembly wrote into OPFS: its bytes were never in wasm memory and never crossed
     * the worker port, and on the browser host they never enter the tab's heap either — a `Blob` on
     * an OPFS file is a handle, and that is what the save is given.
     */
    async #deliverStoredMap(byteLength: number) {
        const blob = await readMapOutput();
        if (blob.size !== byteLength) {
            throw new Error(
                `The assembled map is ${blob.size} bytes in this browser's storage, not the ${byteLength} the ` +
                    `assembler wrote. Free some disk space and try again.`,
            );
        }
        await this.#deliverMap(blob, byteLength);
    }

    /** Deliver the explicitly memory-priced fallback without creating a second full-map Blob. */
    async #deliverResidentMap(bytes: Uint8Array, byteLength: number) {
        if (this.#sinkClosed) return;
        if (bytes.byteLength !== byteLength) {
            throw new Error(
                `The assembler announced ${byteLength} bytes but delivered ${bytes.byteLength}; the map was not sent.`,
            );
        }
        if (this.#output?.kind !== "device") throw new Error("The direct map destination was closed.");
        const { sendMapBytes } = await import("../device/write");
        this.#output.result = await sendMapBytes(this.#output.client, bytes, this.#runFileName, this.#output.ctx);
    }

    /** Deliver the verified one-file result without ever materialising an OPFS
     * map in the tab heap. Device PUT reads the Blob twice in bounded slices
     * (CRC then transfer); browser download hands the same Blob to the browser. */
    async #deliverMap(blob: Blob, byteLength: number) {
        if (this.#sinkClosed) return;
        if (blob.size !== byteLength) {
            throw new Error(
                `The assembler announced ${byteLength} bytes but delivered ${blob.size}; the map was not sent.`,
            );
        }
        const name = this.#runFileName;
        if (this.#output?.kind === "device") {
            const { sendMapBlob } = await import("../device/write");
            this.#output.result = await sendMapBlob(this.#output.client, blob, name, this.#output.ctx);
        } else {
            // No picker here: one file, one ordinary download, one prompt.
            saveBlob(blob, name);
            this.#savedFile = { name, byteLength };
        }
        if (this.#output?.kind !== "device") this.#persisted = true;
    }

    #settleDevice(result: PutResponse | unknown, failed = false) {
        const current = this.#output;
        if (current?.kind !== "device" || current.settled) return;
        current.settled = true;
        current.removeAbort();
        if (failed) current.reject(result);
        else current.resolve(result as PutResponse);
    }

    async #cleanupTransientCells(owner: Output | null = this.#output) {
        const revision = owner?.transientCellRevision ?? null;
        if (owner) owner.transientCellRevision = null;
        if (!revision) return;
        try {
            await discardCellStore(revision);
        } catch {
            this.#runWarnings = [
                ...this.#runWarnings,
                "Temporary map cells could not be deleted; use ‘Delete stored map data’.",
            ];
        }
    }

    async #cleanupDirectOutput() {
        try {
            await discardMapOutput();
        } catch {
            this.#runWarnings = [
                ...this.#runWarnings,
                "The temporary assembled map could not be deleted; use ‘Delete stored map data’.",
            ];
        }
    }

    async setKeepCells(checked: boolean): Promise<void> {
        this.#keepCells = checked;
        this.#cellStorageNotice = null;
        try {
            globalThis.localStorage?.setItem(KEEP_CELLS_KEY, String(checked));
        } catch {
            // The choice still applies to this page; denied storage means it resets to off later.
        }
        if (!checked && !this.#running) await this.clearStoredCells(true);
    }

    async #initializeStorage() {
        const release = await acquireMapWorkStorage();
        try {
            if (this.#destroyed) return;
            if (!this.#keepCells) await clearMapWorkStorage();
        } catch {
            // Startup maintenance is best-effort, as it was before serialization. A later run's
            // own probes give the actionable storage error.
        } finally {
            release();
            if (!this.#destroyed) this.#clearingCells = false;
        }
    }

    async clearStoredCells(announce = true): Promise<void> {
        this.#clearingCells = true;
        if (announce) this.#cellStorageNotice = null;
        const release = await acquireMapWorkStorage();
        try {
            await clearMapWorkStorage();
            if (announce) this.#cellStorageNotice = "Stored map downloads deleted.";
        } catch {
            if (announce) {
                this.#cellStorageNotice =
                    "Stored map downloads could not be deleted. Close other builder tabs and try again.";
            }
        } finally {
            release();
            this.#clearingCells = false;
        }
    }

    #failRun(cause: unknown, runId = this.#activeRunId): Promise<void> {
        if (runId === 0 || this.#activeRunId !== runId || this.#output?.runId !== runId) return this.#failure;
        this.#failure = this.#failRunOwned(cause, runId);
        return this.#failure;
    }

    async #failRunOwned(cause: unknown, runId: number) {
        // A cancelled preflight may resume after an unabortable OPFS call. Its
        // continuation no longer owns any shared run state and must be a no-op.
        const owner = this.#output;
        if (runId === 0 || this.#activeRunId !== runId || owner?.runId !== runId) return;
        const direct = owner.kind === "device";
        if (owner.kind === "device") {
            if (owner.failing || owner.settled) return;
            owner.failing = true;
        }
        this.#activeRunId = 0;
        const controller = this.#abortCtl;
        this.#abortCtl = null;
        controller?.abort();
        this.#worker?.terminate();
        this.#worker = null;
        // Close the logical sink before the first await. A PUT can finish on the same microtask
        // turn as cancellation; its `done` handler must already know that failure teardown won.
        this.#sinkClosed = true;
        // `clearCellStores`, OPFS probes/opens and cell writes cannot be interrupted. Keep the
        // surface in its running state until every operation owned by this run has physically
        // returned; only then may cleanup run and the next map action become available.
        await waitForRunOperations(owner);
        // A delivery that has not begun is now a no-op, but one may already be in
        // flight — let it land before cleanup. It is bounded by one file's write.
        const deliveryCause = await this.#delivery.then(
            () => null,
            (error: unknown) => error,
        );
        // Teardown cancellation can arrive while the physical link rejection is already
        // unwinding. Preserve that stable cause so the DeviceJob can surface interruption.
        if ((deliveryCause as { code?: unknown } | null)?.code === "link") cause = deliveryCause;
        const cancelled = cause instanceof DOMException && cause.name === "AbortError";
        await this.#cleanupTransientCells(owner);
        if (direct) await this.#cleanupDirectOutput();
        owner.releaseWork?.();
        owner.releaseWork = null;
        this.#errorMessage = cause instanceof Error ? cause.message : String(cause);
        this.#phase = cancelled ? "cancelled" : "error";
        this.#settleDevice(cause, true);
        this.#output = null;
    }

    #assertRunActive(runId: number, out: Output, controller: AbortController) {
        if (
            this.#activeRunId !== runId ||
            this.#output !== out ||
            this.#abortCtl !== controller ||
            controller.signal.aborted
        ) {
            throw controller.signal.reason ?? new DOMException("the map run is no longer active", "AbortError");
        }
    }

    async #runOperation<T>(
        runId: number,
        out: Output,
        controller: AbortController,
        operation: () => Promise<T>,
        onStale?: (value: T) => Promise<void>,
    ): Promise<T> {
        this.#assertRunActive(runId, out, controller);
        let tracked!: Promise<T>;
        tracked = (async () => {
            try {
                const value = await operation();
                try {
                    this.#assertRunActive(runId, out, controller);
                } catch (cause) {
                    // Some operations create an artifact before cancellation can be observed.
                    // Their caller owns the value/revision needed to remove it, and that cleanup
                    // remains inside this tracked promise so failRun's barrier includes it.
                    await onStale?.(value);
                    throw cause;
                }
                return value;
            } finally {
                out.operations.delete(tracked);
            }
        })();
        out.operations.add(tracked);
        return tracked;
    }

    /** What the selection is called in the file and on the device. */
    #mapName(): string {
        const parts = this.#store.selection.parts;
        const base = parts.length === 0 ? "OBC map" : parts[0].name;
        const name = parts.length > 1 ? `${base} +${parts.length - 1}` : base;
        return truncateUtf8(name, 48);
    }

    /**
     * The map's filename. **JS owns this**: the assembler names nothing — what crosses its seam is
     * a digest and a length — so the name is chosen here, where the selection is known. `.obcm`
     * because that is what a map is, and what this app's own file picker accepts. What an OS
     * forbids in a name becomes a dash.
     */
    #mapFileName(): string {
        const stem = this.#mapName().replace(/[\\/:*?"<>|]/g, "-").trim();
        return `${stem.length > 0 ? stem : "OBC map"}.obcm`;
    }

    async #begin(out: Output) {
        const store = this.#store;
        const resolution = store.resolution;
        const indices = store.indices;
        const l = this.#ledger;
        if (!resolution || !indices || !l || !this.#ready || !this.#estimate?.fits) {
            throw new Error("This map is not ready to assemble yet.");
        }
        const admittedEstimate = this.#estimate;
        const admissionId = this.#estimateGeneration;
        const requireDisk = this.#estimateOnDisk;
        if (this.#destroyed) throw new DOMException("the map builder was closed", "AbortError");
        if (this.#estimateGeneration !== admissionId || this.#estimate !== admittedEstimate || !this.#ready) {
            throw new Error("The map selection changed. Wait for its memory check before trying again.");
        }
        const runId = ++this.#nextRunId;
        out.runId = runId;
        this.#activeRunId = runId;
        this.#output = out;
        this.#lastRunKind = out.kind;
        this.#runFileName = this.#mapFileName();
        this.#errorMessage = null;
        this.#savedFile = null;
        this.#persisted = false;
        this.#delivery = Promise.resolve();
        this.#sinkClosed = false;
        this.#runWarnings = [];
        this.#dlProgress = null;
        this.#asmFraction = 0;
        this.#readMode = null;
        this.#writeMode = null;
        this.#cachedCells = 0;
        this.#cachedBytes = 0;
        this.#phase = "downloading";
        if (out.kind === "device") out.ctx.phase("downloading", l.totalBytes);

        const plan = planCells(resolution, store.catalog, indices, store.terrain, store.articles);
        const cells: WorkerCell[] = [];
        const sourceCells: WorkerSourceCell[] = [];
        const terrainCells: WorkerTerrainCell[] = [];
        const articles = { landmarks: [] as Uint8Array[], peaks: [] as Uint8Array[] };
        const runAbort = new AbortController();
        this.#abortCtl = runAbort;
        const assertActive = () => this.#assertRunActive(runId, out, runAbort);
        const runOp = <T>(operation: () => Promise<T>, onStale?: (value: T) => Promise<void>) =>
            this.#runOperation(runId, out, runAbort, operation, onStale);

        // Readiness only covers this instance. The lease also waits for an older,
        // already-destroyed instance's put/finalization before this run touches shared OPFS.
        out.releaseWork = await runOp(acquireMapWorkStorage, async (release) => release());

        // Cells go to disk when this browser will take them, which keeps a country's
        // worth out of the tab's heap, and with the opt-in above a later build can
        // reuse them. The raster deliberately does not: its objects are small and it
        // is downloaded last. Probed by writing and reading a file back, not by
        // sniffing for a method: a store that cannot be written to is worse than none.
        const revision = cellStoreRevision(store.catalog);
        // Unchecked means no reuse in either direction. OPFS still carries this run when available,
        // because that working disk is what makes country-sized assembly possible.
        if (!this.#keepCells) {
            await runOp(clearCellStores);
        }
        const writable = await runOp(cellStoreWritable);
        out.transientCellRevision = writable && !this.#keepCells ? revision : null;
        let cellStore = writable
            ? await runOp(
                  () => openCellStore(revision),
                  async () => {
                      if (!out.transientCellRevision) return;
                      try {
                          await discardCellStore(out.transientCellRevision);
                          out.transientCellRevision = null;
                      } catch {
                          // failRun owns the same revision and retries after this barrier drains.
                      }
                  },
              )
            : null;
        if (!cellStore) out.transientCellRevision = null;
        let fetchPlan = plan;
        if (cellStore) {
            const cached = await scanCached(plan, cellStore, runOp);
            fetchPlan = cached.plan;
            this.#cachedCells = cached.cells;
            this.#cachedBytes = cached.bytes;
            // Asked once, before a byte is fetched, and about the WHOLE run — cells, the
            // map the sink writes, the merge's spill — because all three live in OPFS: a
            // store with room for the cells but not the output would fail at the first
            // write, after the download.
            const reclaimable = await runOp(reclaimableAssemblyBytes);
            const hasRoom = await runOp(() => hasRoomFor(projectedRunDiskBytes(l, this.#cachedBytes), reclaimable));
            if (!hasRoom) {
                cellStore = null;
                fetchPlan = plan;
                this.#cachedCells = 0;
                this.#cachedBytes = 0;
            } else {
                // Only an admitted run replaces the previous temporary output. A refused run leaves
                // it alone, and accepted work reclaims the bytes before the first download.
                await runOp(clearAssemblyStorage);
            }
        }

        if (requireDisk && !cellStore) {
            throw new Error("The storage required for this map is no longer available. Retry the memory check.");
        }

        try {
            await runOp(() =>
                downloadCells(fetchPlan, {
                    fetchImpl: store.client.fetchImpl,
                    onCell: async (item, bytes) => {
                        if (item.article) {
                            articles[item.article].push(bytes);
                        } else if (item.band === null) {
                            terrainCells.push({ id: item.cell.id, sha256: item.cell.sha256, bytes });
                        } else if (cellStore) {
                            // Verified once, on the way in: the file's name is the digest
                            // `fetchVerified` just checked. Awaited, so a slow disk applies
                            // backpressure instead of letting gigabytes queue behind it.
                            await runOp(() =>
                                cellStore.put(item.cell.sha256, bytes).catch((cause: unknown) => {
                                    if (isStorageQuotaError(cause)) throw new Error(STORAGE_QUOTA_MESSAGE);
                                    throw new Error(
                                        `The map could not be written to this browser's storage (${
                                            cause instanceof Error ? cause.message : String(cause)
                                        }).`,
                                    );
                                }),
                            );
                        } else {
                            cells.push({
                                id: item.cell.id,
                                band: item.band,
                                partial: "partial" in item.cell && item.cell.partial,
                                bytes,
                            });
                        }
                    },
                    onProgress: (p) => {
                        this.#dlProgress = p;
                        if (out.kind === "device") out.ctx.progress(p.receivedBytes, p.totalBytes);
                    },
                    signal: runAbort.signal,
                }),
            );
            // In the plan's order, not the network's — cached and fetched cells
            // are one list, and which of the two a cell came from must not be
            // able to reach the bytes.
            if (cellStore) {
                for (const item of plan.items) {
                    if (item.band === null) continue;
                    sourceCells.push({
                        id: item.cell.id,
                        band: item.band,
                        partial: "partial" in item.cell && item.cell.partial,
                        byteLength: item.cell.bytes,
                        key: item.cell.sha256,
                    });
                }
            }
        } catch (e) {
            await this.#failRun(e, runId);
            return;
        }

        this.#phase = "assembling";
        if (out.kind === "device") out.ctx.phase("assembling", 0);
        this.#asmPhase = "open";
        this.#asmPhaseStart = { open: performance.now() };
        const req: AssembleWorkerRequest = {
            type: "assemble",
            requireDisk,
            cells,
            // One or the other, never a mix: `cells` carries buffers when there was
            // nowhere to put them, `sourceCells` names files in OPFS when there was.
            // The worker decides how it reads the latter.
            sourceCells: cellStore ? sourceCells : undefined,
            cellStore: cellStore?.revision,
            knownEmpty: plan.knownEmpty,
            // Terrain travels as its own pair: the lattice the catalog states and the
            // objects that were downloaded. A catalog with no terrain block sends
            // neither, and the map is written with an empty terrain region.
            terrain: store.terrain
                ? { postingLog2: store.terrain.posting_log2, cellLog2: store.terrain.cell_log2 }
                : undefined,
            terrainCells: store.terrain ? terrainCells : undefined,
            articles,
            schemaJson: store.rootBody,
            lightSkinJson: JSON.stringify(store.lightSkin),
            darkSkinJson: JSON.stringify(store.darkSkin),
            options: {
                // The same budget the estimate was given, so the projection prices
                // the run that actually happens rather than the engine's default.
                mergeBudgetBytes: SORT_BUDGET_BYTES,
                // Both were shown before this button unlocked. `acceptHoles` is derived
                // from the *shown* set, not the ledger's raw count, so the assembly is
                // never told to accept a hole the UI kept to itself. Partial cells exist
                // in essentially every real map.
                acceptHoles: store.holeCells().length > 0,
                acceptPartial: true,
            },
        };
        assertActive();
        this.#ensureWorker().postMessage(req, { transfer: requestTransferList(req) });
    }

    download(): void {
        const out: DownloadOutput = {
            kind: "download",
            runId: null,
            transientCellRevision: null,
            releaseWork: null,
            operations: new Set(),
        };
        void this.#begin(out).catch((cause) => this.#failRun(cause, out.runId ?? 0));
    }

    /** Assemble this selection and stream its verified `.obcm` directly into
     * the connected device's v4 flat-store PUT. The Blob is OPFS-backed when
     * this host passed the writable-storage probe, and memory-priced otherwise. */
    readonly sendToDevice: SendAssembledMap = (client, ctx) =>
        new Promise<PutResponse>((resolve, reject) => {
            const onAbort = () =>
                void this.#failRun(
                    ctx.signal.reason ?? new DOMException("cancelled", "AbortError"),
                    device.runId ?? 0,
                );
            const device: DeviceOutput = {
                kind: "device",
                client,
                ctx,
                resolve,
                reject,
                removeAbort: () => ctx.signal.removeEventListener("abort", onAbort),
                result: null,
                settled: false,
                failing: false,
                runId: null,
                transientCellRevision: null,
                releaseWork: null,
                operations: new Set(),
            };
            ctx.signal.addEventListener("abort", onAbort, { once: true });
            if (ctx.signal.aborted) {
                device.removeAbort();
                reject(ctx.signal.reason);
                return;
            }
            void this.#begin(device).catch((cause) => {
                if (this.#output === device) void this.#failRun(cause, device.runId ?? 0);
                else {
                    device.removeAbort();
                    reject(cause);
                }
            });
        });

    readonly #refreshBlocked = $derived(this.#output?.kind === "device" || this.#phase === "finalizing");

    /** Quiesce this catalog before a replacement can admit another assembly. */
    async pauseForRefresh(): Promise<() => void> {
        if (this.#refreshBlocked) throw new Error("Wait for the map send or cleanup to finish before refreshing.");
        this.#paused = true;
        this.#estimateGeneration += 1;
        await this.#failRun(new DOMException("the map catalog is being refreshed", "AbortError"));
        this.#worker?.terminate();
        this.#worker = null;
        return () => { this.#paused = false; };
    }

    cancel(): void {
        const cause = new DOMException("cancelled", "AbortError");
        // Direct assembly and PUT are one DeviceJob. Ask its controller to
        // cancel so both this button and TransferBar cancel the same signal;
        // merely aborting the cell downloader would let a live PUT commit and
        // only then paint the run as cancelled.
        if (this.#output?.kind === "device") {
            this.#output.ctx.cancel(cause);
            return;
        }
        if (this.#phase === "downloading") {
            this.#abortCtl?.abort(cause);
        } else if (this.#phase === "assembling" || this.#phase === "saving") {
            // The worker is blocked inside one synchronous wasm call and cannot read
            // a message — terminate IS the cancel. Nothing usable is left behind: a
            // partial `.obcm` fails its own header checks, and the file is only saved
            // once the run says it finished.
            void this.#failRun(cause);
        }
    }

    retryEstimate(): void {
        this.#estimateNonce += 1;
    }

    #estimateEffect() {
        void this.#estimateNonce; // the retry button's lever
        const l = this.#ledger;
        const estimateId = ++this.#estimateGeneration;
        this.#estimate = null;
        this.#estimateLedger = null;
        const phase = this.#phase;
        const idle = phase === "idle" || phase === "done" || phase === "cancelled" || phase === "error";
        if (!l || !l.isFinal || l.cellCount === 0 || !idle || this.#clearingCells || this.#paused) {
            // Every exit clears the pending flag: a selection that empties or a run
            // that starts must not leave "waiting for an estimate" latched with
            // nothing left to answer it.
            this.#estimate = null;
            this.#estimatePending = false;
            return;
        }
        const networkBandBytes = l.core.bytes;
        const totalCellBytes = l.totalBytes;
        // Both artifact classes stay resident even when map cells stream from disk.
        const terrainBytes = (l.terrain?.bytes ?? 0) + (l.articleBytes ?? 0);
        const reuseCells = this.#keepCells;
        const store = this.#store;
        const resolution = store.resolution;
        const indices = store.indices;
        const catalog = store.catalog;
        const plan = reuseCells && resolution && indices
            ? planCells(resolution, catalog, indices, store.terrain, store.articles)
            : null;
        const revision = cellStoreRevision(catalog);
        this.#estimatePending = true;
        this.#estimateError = null;
        // Debounced: a slider mid-drag changes the figures every frame, and the
        // projection only matters once the selection settles.
        const timer = setTimeout(() => {
            // The main thread's half of both residency escapes, decided by the same
            // two checks `begin` runs before a byte is fetched: a store this browser
            // will write, with room for the WHOLE run. The worker ANDs in its own
            // sync-handle probe.
            void (async () => {
                const writable = await cellStoreWritable();
                let storedCellBytes = 0;
                if (writable && plan) {
                    const cells = await openCellInventory(revision);
                    if (cells) storedCellBytes = (await scanCached(plan, cells)).bytes;
                }
                const reclaimable = writable ? await reclaimableAssemblyBytes() : 0;
                const diskNeed = projectedRunDiskBytes(l, storedCellBytes);
                const onDisk = writable && (await hasRoomFor(diskNeed, reclaimable));
                if (this.#destroyed || estimateId !== this.#estimateGeneration || l !== this.#ledger) return;
                this.#estimateLedger = l;
                this.#ensureWorker().postMessage({
                    type: "estimate",
                    estimateId,
                    networkBandBytes,
                    totalCellBytes,
                    terrainBytes,
                    onDisk,
                    mergeBudgetBytes: SORT_BUDGET_BYTES,
                    budgetBytes: isMobileUa ? MOBILE_BUDGET : undefined,
                } satisfies AssembleWorkerRequest);
            })();
        }, 500);
        return () => {
            clearTimeout(timer);
            if (estimateId === this.#estimateGeneration) this.#estimateGeneration += 1;
        };
    }

    readonly #memoryRefusal = $derived.by(() => {
        const estimate = this.#estimate;
        if (!estimate || estimate.fits) return null;
        return (
            `Assembling this selection needs about ${formatBytes(estimate.peakBytes)} of browser memory — more than ` +
            `${isMobileUa ? "a phone's tab" : "a browser tab"} can be trusted with ` +
            // A card carries **one** map, because the device has no way to choose
            // between two. So "split it in two" is not a remedy that exists here,
            // and the only honest instruction is to cover less ground.
            `(${formatBytes(estimate.budgetBytes)}). Reduce the map selection.`
        );
    });

    readonly #memoryCaution = $derived.by(() => {
        const estimate = this.#estimate;
        if (!estimate || !estimate.fits) return null;
        if (estimate.headroomBytes >= estimate.budgetBytes * 0.15) return null;
        return (
            `Close to the browser's memory budget: about ${formatBytes(estimate.peakBytes)} projected of ` +
            `${formatBytes(estimate.budgetBytes)}. It will probably assemble; a smaller area is the sure thing.`
        );
    });

    readonly #running = $derived(
        this.#phase === "downloading" || this.#phase === "assembling" || this.#phase === "saving" || this.#phase === "finalizing",
    );
    readonly #refusal = $derived.by(() => {
        const l = this.#ledger;
        if (!l || l.cellCount === 0) return null;
        return this.#memoryRefusal;
    });
    readonly #ready = $derived.by(() => {
        const l = this.#ledger;
        return (
            l !== null &&
            l.isFinal &&
            l.cellCount > 0 &&
            this.#refusal === null &&
            !this.#running &&
            !this.#paused &&
            !this.#clearingCells &&
            this.#estimate !== null &&
            this.#estimateLedger === l &&
            !this.#estimatePending &&
            // An unanswered projection keeps the mandatory pre-download check honest:
            // the button waits for the retry, not forever.
            this.#estimateError === null
        );
    });
}

async function waitForRunOperations(owner: Output) {
    // Operations can start nested work (download callbacks start OPFS writes), so drain until
    // the owned set remains empty after a microtask turn rather than trusting one snapshot.
    for (;;) {
        const pending = [...owner.operations];
        if (pending.length > 0) await Promise.allSettled(pending);
        await Promise.resolve();
        if (owner.operations.size === 0) return;
    }
}
