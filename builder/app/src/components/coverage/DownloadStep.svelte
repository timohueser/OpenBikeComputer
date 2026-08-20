<script lang="ts">
    // Final coverage proof, then delivery: cells fetched and verified against
    // the catalog, assembled into ONE `.obcm` by wasm in a Worker, and either
    // downloaded normally or streamed straight to a connected device.
    //
    // A map is one file, which decides the shape of this screen's second half. There
    // is nothing to order, nothing to package and nothing to acknowledge: the run
    // produces a single object, and the only question left is where it lands.

    import { onDestroy, onMount } from "svelte";
    import type { AssemblePhase, MemoryEstimate } from "../../lib/assemble/bridge";
    import {
        isWorkerResponse,
        requestTransferList,
        type AssembleWorkerRequest,
        type CellReadMode,
        type MapWriteMode,
        type WorkerCell,
        type WorkerSourceCell,
        type WorkerTerrainCell,
    } from "../../lib/assemble/workerProtocol";
    import {
        cellStoreRevision,
        cellStoreWritable,
        clearCellStores,
        clearMapWorkStorage,
        discardCellStore,
        hasRoomFor,
        openCellStore,
        readMapOutput,
        type CellStore,
    } from "../../lib/cells/store";
    import { saveBlob } from "../../lib/download";
    import { platform } from "../../lib/platform";
    import type { MapOutputSession } from "../../lib/platform/types";
    import {
        downloadCells,
        planCells,
        type CellDownloadPlan,
        type CellDownloadProgress,
    } from "../../lib/catalog/download";
    import { coverageRings, type RingPoint } from "../../lib/catalog/outline";
    import type { UBox } from "../../lib/catalog/grid";
    import { detailBandId, mergeMixedCellRects, parseCells, patchCount } from "../../lib/coverage/shape";
    import type { CoverageStore } from "../../lib/coverage/store.svelte";
    import type { JobContext } from "../../lib/device/progress";
    import type { SendAssembledMap } from "../../lib/device/write";
    import { formatBytes, truncateUtf8 } from "../../lib/format";
    import type { FlatStoreClient } from "../../lib/usb/client";
    import type { PutResponse } from "../../lib/usb/protocol";

    let {
        store,
        onSendReadyChange,
    }: { store: CoverageStore; onSendReadyChange?: (ready: boolean) => void } = $props();

    const KEEP_CELLS_KEY = "obcm.keepMapCells";

    function loadKeepCells(): boolean {
        try {
            return globalThis.localStorage?.getItem(KEEP_CELLS_KEY) === "true";
        } catch {
            return false;
        }
    }

    /** The engine's sort budget (#1116 phase D), which after the external merge **is** the wasm
     *  engine term. 256 MB: big enough that a DACH-scale sort is ~a dozen runs (well inside the
     *  scratch pool), small enough that engine + caches + terrain stays a fraction of a tab's
     *  budget. Passed to both the assembly and the estimate, so the projection prices the run
     *  that will actually happen. */
    const SORT_BUDGET_BYTES = 256 * 1024 * 1024;

    /** What a run of this ledger needs from OPFS, all three tenants together: the cells (minus
     *  terrain, which never goes to disk), the assembled map the sink writes (≈ the cells, measured
     *  1.00), and the merge's spill (edge + adjacency + claim streams; ≤ 2.5× the network band,
     *  the estimate model's own coefficient). Used by the estimate effect and by `begin`, so the
     *  projection and the run decide OPFS-or-fallback by the same arithmetic — a run that can
     *  store its cells but not its output would otherwise fail at the first write, after the
     *  download. */
    function runDiskNeed(l: { totalBytes: number; core: { bytes: number }; terrain: { bytes: number } | null }) {
        const terrain = l.terrain?.bytes ?? 0;
        return l.totalBytes - terrain + l.totalBytes + 2.5 * l.core.bytes;
    }

    const ledger = $derived(store.ledger);
    const detailBand = $derived(detailBandId(store.catalog));

    // --- worker lifetime --------------------------------------------------

    let worker: Worker | null = null;

    function ensureWorker(): Worker {
        if (!worker) {
            worker = new Worker(new URL("../../lib/assemble/assemble.worker.ts", import.meta.url), {
                type: "module",
            });
            worker.onmessage = onWorkerMessage;
            // The worker's own code answers every failure with an `error`
            // message — these two fire for what that code never sees: the
            // script failing to boot at all (a lost chunk after a deploy) and
            // a message that could not be deserialized. Both arrive as the
            // protocol's `{code: "internal"}` shape so one switch handles
            // every failure, and the worker is dropped — a worker that could
            // not boot stays broken, so the next request must spawn fresh.
            worker.onerror = (e) => {
                workerFailed(e.message || "the assembly worker failed to start");
            };
            worker.onmessageerror = () => {
                workerFailed("a message to the assembly worker could not be delivered");
            };
        }
        return worker;
    }

    /** A worker-level failure (#1041 A4), routed like a posted `error`. */
    function workerFailed(message: string) {
        worker?.terminate();
        worker = null;
        onWorkerMessage(
            new MessageEvent("message", { data: { type: "error", code: "internal", message } }),
        );
    }

    onDestroy(() => {
        const cause = new DOMException("the map builder was closed", "AbortError");
        if (output?.kind === "device" && !output.settled) output.ctx.cancel(cause);
        else abortCtl?.abort(cause);
        worker?.terminate();
        void closeDownloadOutput(true);
        void cleanupTransientCells();
        onSendReadyChange?.(false);
    });

    // Releases before the opt-in retained cells automatically. The first visit to this step after
    // the change removes that legacy cache unless the rider has explicitly enabled reuse.
    onMount(() => {
        if (!keepCells) void clearStoredCells(false);
    });

    // --- the run ----------------------------------------------------------

    type Phase = "idle" | "downloading" | "assembling" | "saving" | "done" | "cancelled" | "error";
    let phase = $state<Phase>("idle");
    let dlProgress = $state<CellDownloadProgress | null>(null);
    let asmPhase = $state<AssemblePhase>("open");
    let asmFraction = $state(0);
    /** The map, once there is one: what it is called, how big it is, and where it
     *  went if the host had somewhere to put it. */
    let savedFile = $state<{ name: string; byteLength: number; path?: string } | null>(null);
    /** Whether the file actually reached the disk. Not `savedFile !== null`: it is
     *  set the moment the map is delivered, and a cancel that raced the delivery
     *  must not tell someone to go and discard a file nobody has. */
    let persisted = $state(false);
    /** The one delivery, so `done` can wait for a save/send that is still running rather
     *  than declare the map finished behind it. */
    let delivery: Promise<void> = Promise.resolve();
    /** Set by `failRun`: a delivery that has not started must not write into an
     *  output that is being discarded. */
    let sinkClosed = false;
    /** How this run's cells reached the assembler (#1116 B2), as the worker
     *  reports it before the assembly starts. Shown, because a bug report about a
     *  failed country-scale run has to say which path ran. */
    let readMode = $state<CellReadMode | null>(null);
    /** …and where its map went (#1116 D1), for the same reason. */
    let writeMode = $state<MapWriteMode | null>(null);
    /** Cells this run did not have to fetch, because a previous one already put
     *  them in OPFS under the same digest. */
    let cachedCells = $state(0);
    let cachedBytes = $state(0);
    /** Opt-in reuse. OPFS may still be the active run's necessary working disk when false. */
    let keepCells = $state(loadKeepCells());
    let clearingCells = $state(false);
    let cellStorageNotice = $state<string | null>(null);
    /** A revision this run must remove after every reader has closed. */
    let transientCellRevision: string | null = null;
    let outputPath = $state<string | null>(null);
    let runWarnings = $state<string[]>([]);
    let errorMessage = $state<string | null>(null);
    let abortCtl: AbortController | null = null;
    let downloadOutput: MapOutputSession | null = null;
    let outputCleanupFailed = $state(false);
    /** The name this run's file will be saved under, fixed when the run starts. Read
     *  once rather than at save time, so editing the selection mid-download cannot
     *  rename the map that is already being built from the old one.
     *
     *  `$state` because the saving row renders it: it is written during a run, and a
     *  plain `let` would leave that row showing the previous run's name. */
    let runFileName = $state("OBC map.obcm");

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
    }
    type Output = { readonly kind: "download" } | DeviceOutput;
    let output = $state.raw<Output | null>(null);
    let lastRunKind = $state<Output["kind"]>("download");

    const ASM_PHASE_LABEL: Record<AssemblePhase, string> = {
        open: "reading cells",
        poi: "merging places",
        nav: "stitching the road network",
        plan: "planning the file",
        write: "writing",
        verify: "checking the result",
        done: "done",
    };

    async function onWorkerMessage(e: MessageEvent) {
        const msg = e.data as unknown;
        if (!isWorkerResponse(msg)) return;
        switch (msg.type) {
            case "estimate-result":
                estimate = msg.estimate;
                estimatePending = false;
                estimateError = null;
                break;
            case "progress":
                asmPhase = msg.phase;
                asmFraction = msg.fraction;
                break;
            case "reading":
                readMode = msg.mode;
                break;
            case "writing":
                writeMode = msg.mode;
                break;
            case "stored-map":
                // The map never crossed the port: it is in OPFS, the worker has let
                // go of it, and this is where it becomes a file someone has. The
                // save is started here and awaited by `done`, because a multi-
                // gigabyte write outlives this handler.
                // **The phase moves before the delivery is assigned.** A ~9 GiB write
                // outlives this handler, and while it ran the screen still said
                // "assembling" — so a rider who pressed Cancel during the save was
                // cancelling a thing that had already finished, and the button's own
                // branch dispatched on the wrong phase.
                phase = "saving";
                delivery = deliverStoredMap(msg.byteLength);
                void delivery.catch(() => {});
                break;
            case "file":
                // No sink was available, so the bytes came across instead. Same
                // delivery, one wrap earlier.
                phase = "saving";
                delivery = deliverMap(
                    new Blob([msg.bytes as unknown as BlobPart]),
                    msg.byteLength,
                    msg.bytes,
                );
                void delivery.catch(() => {});
                break;
            case "done":
                // The worker's OPFS ledger, for anyone profiling an assembly
                // from DevTools — a worker's own console does not surface.
                if (msg.io) console.debug("[assemble] opfs i/o", msg.io);
                runWarnings = msg.warnings;
                try {
                    await delivery;
                    await closeDownloadOutput(false);
                } catch (cause) {
                    await failRun(cause);
                    break;
                }
                // A cancel that arrived *during* the save has already discarded the
                // output and set the phase — `failRun` awaits this same delivery, so
                // both paths resume here. Without this guard the later writer won:
                // a discarded map could report "done", and the run that kept its file
                // could report "Cancelled". `sinkClosed` is the fact that settles it,
                // because it is set before the discard rather than after it.
                if (sinkClosed) break;
                phase = "done";
                await cleanupTransientCells();
                if (output?.kind === "device") {
                    if (!output.result) {
                        await failRun(new Error("The device did not commit the assembled map."));
                        break;
                    }
                    settleDevice(output.result);
                }
                output = null;
                break;
            case "error":
                // Two conversations share this worker, and their failures are
                // different facts (#1041 A3): an error during a run belongs to
                // the run, but an error answering the background *estimate*
                // must not paint the screen with "Nothing was saved" about a
                // download that never started — it gets its own channel and
                // its own retry, and never wedges the button behind a pending
                // flag nothing will clear.
                if (phase === "assembling" || phase === "saving") {
                    await failRun(new Error(msg.message));
                } else {
                    estimateError = msg.message;
                }
                estimatePending = false;
                break;
        }
    }

    /**
     * The map the assembly wrote into OPFS (#1116 D1): its bytes were never in wasm
     * memory and never crossed the worker port, and on the browser host they never
     * enter the tab's heap either — a `Blob` on an OPFS file is a handle, and that is
     * what the save is given.
     *
     * The desktop host does read it back, because its output session takes bytes to
     * write into a real folder. That is one map resident, which is the residency this
     * host always had; the saving D1 is here for is on the *assembly*, and it is
     * unaffected.
     */
    async function deliverStoredMap(byteLength: number) {
        const blob = await readMapOutput();
        if (blob.size !== byteLength) {
            throw new Error(
                `The assembled map is ${blob.size} bytes in this browser's storage, not the ${byteLength} the ` +
                    `assembler wrote. Free some disk space and try again.`,
            );
        }
        await deliverMap(blob, byteLength, null);
    }

    /** Deliver the verified one-file result without ever materialising an OPFS
     * map in the tab heap. Device PUT reads the Blob twice in bounded slices
     * (CRC then transfer); browser download hands the same Blob to the browser. */
    async function deliverMap(blob: Blob, byteLength: number, bytes: Uint8Array | null) {
        if (sinkClosed) return;
        if (blob.size !== byteLength) {
            throw new Error(
                `The assembler announced ${byteLength} bytes but delivered ${blob.size}; the map was not sent.`,
            );
        }
        const name = runFileName;
        if (output?.kind === "device") {
            const { sendMapBlob } = await import("../../lib/device/write");
            output.result = await sendMapBlob(output.client, blob, name, output.ctx);
        } else if (downloadOutput) {
            // A picked directory (or the desktop's native folder) takes the map
            // where the rider wants it — the card itself, when that is what they
            // picked. The session streams a Blob without buffering it; only a host
            // that needs contiguous bytes converts, on its side.
            const path = await downloadOutput.write(name, bytes ?? blob);
            savedFile = { name, byteLength, path };
        } else {
            // No picker here: one file, one ordinary download, one prompt.
            saveBlob(blob, name);
            savedFile = { name, byteLength };
        }
        if (output?.kind !== "device") persisted = true;
    }

    function settleDevice(result: PutResponse | unknown, failed = false) {
        const current = output;
        if (current?.kind !== "device" || current.settled) return;
        current.settled = true;
        current.removeAbort();
        if (failed) current.reject(result);
        else current.resolve(result as PutResponse);
    }

    async function closeDownloadOutput(discard: boolean) {
        const current = downloadOutput;
        downloadOutput = null;
        if (current) await (discard ? current.discard() : current.finish());
    }

    async function cleanupTransientCells() {
        const revision = transientCellRevision;
        transientCellRevision = null;
        if (!revision) return;
        try {
            await discardCellStore(revision);
        } catch {
            runWarnings = [
                ...runWarnings,
                "Temporary map cells could not be deleted; use ‘Delete stored map data’.",
            ];
        }
    }

    async function setKeepCells(checked: boolean) {
        keepCells = checked;
        cellStorageNotice = null;
        try {
            globalThis.localStorage?.setItem(KEEP_CELLS_KEY, String(checked));
        } catch {
            // The choice still applies to this page; denied storage means it resets to off later.
        }
        if (!checked && !running) await clearStoredCells(true);
    }

    async function clearStoredCells(announce = true) {
        clearingCells = true;
        if (announce) cellStorageNotice = null;
        try {
            await clearMapWorkStorage();
            if (announce) cellStorageNotice = "Stored map downloads deleted.";
        } catch {
            if (announce) {
                cellStorageNotice =
                    "Stored map downloads could not be deleted. Close other builder tabs and try again.";
            }
        } finally {
            clearingCells = false;
        }
    }

    async function failRun(cause: unknown) {
        if (output?.kind === "device") {
            if (output.failing || output.settled) return;
            output.failing = true;
        }
        const cancelled = cause instanceof DOMException && cause.name === "AbortError";
        abortCtl?.abort();
        worker?.terminate();
        worker = null;
        // A delivery that has not begun is now a no-op, but one may already be in
        // flight — let it land before the output is discarded, or the discard races
        // the write into the folder it removes. It is bounded by one file's write.
        sinkClosed = true;
        await delivery.catch(() => {});
        await closeDownloadOutput(true).catch(() => (outputCleanupFailed = true));
        await cleanupTransientCells();
        errorMessage = cause instanceof Error ? cause.message : String(cause);
        phase = cancelled ? "cancelled" : "error";
        settleDevice(cause, true);
        output = null;
    }

    /**
     * The same plan minus every cell already in the store, and the tally of what
     * that saved.
     *
     * A cell is "already here" when a file named by its catalog digest exists at
     * the catalog's length. That is the whole identity check: the name **is** the
     * SHA-256 `fetchVerified` matched before the file was written, and the length
     * catches the one thing that can go wrong afterwards — a write torn by a
     * crash or a quota refusal. Re-hashing every cell on every run would cost
     * seconds and a full read of exactly the bytes this exists to stop reading.
     *
     * Terrain is never skipped: it is not in the store (see `begin`).
     */
    async function skipCached(
        plan: CellDownloadPlan,
        cells: CellStore,
    ): Promise<CellDownloadPlan> {
        const wanted: typeof plan.items = [];
        let bytes = 0;
        let have = 0;
        let haveBytes = 0;
        for (const item of plan.items) {
            if (item.band !== null && (await cells.has(item.cell.sha256, item.cell.bytes))) {
                have += 1;
                haveBytes += item.cell.bytes;
                continue;
            }
            wanted.push(item);
            bytes += item.cell.bytes;
        }
        cachedCells = have;
        cachedBytes = haveBytes;
        return { ...plan, items: wanted, totalBytes: bytes };
    }

    /** What the selection is called in the file and on the device. */
    function mapName(): string {
        const parts = store.selection.parts;
        const base = parts.length === 0 ? "OBC map" : parts[0].name;
        const name = parts.length > 1 ? `${base} +${parts.length - 1}` : base;
        return truncateUtf8(name, 48);
    }

    /**
     * The map's filename. **JS owns this**: the assembler names nothing — what
     * crosses its seam is a digest and a length — so the name is chosen here, where
     * the selection is known.
     *
     * `.obcm` because that is what the device scans a card for, and what its own
     * file picker accepts. What an OS forbids in a name becomes a dash.
     */
    function mapFileName(): string {
        const stem = mapName().replace(/[\\/:*?"<>|]/g, "-").trim();
        return `${stem.length > 0 ? stem : "OBC map"}.obcm`;
    }

    async function begin(out: Output) {
        const resolution = store.resolution;
        const indices = store.indices;
        const l = ledger;
        if (!resolution || !indices || !l || !ready) {
            throw new Error("This map is not ready to assemble yet.");
        }
        // Native desktop saving still opens its output session under the click
        // that started a download. Direct device delivery never opens a save
        // destination, and the web host uses an ordinary browser download.
        let picked: MapOutputSession | null = null;
        if (out.kind === "download" && platform.openMapOutput) {
            try {
                picked = await platform.openMapOutput(mapName());
            } catch (cause) {
                if (cause instanceof DOMException && cause.name === "AbortError") return;
                throw cause;
            }
        }
        output = out;
        lastRunKind = out.kind;
        runFileName = mapFileName();
        errorMessage = null;
        savedFile = null;
        persisted = false;
        delivery = Promise.resolve();
        sinkClosed = false;
        outputPath = picked?.path ?? null;
        downloadOutput = picked;
        outputCleanupFailed = false;
        runWarnings = [];
        dlProgress = null;
        asmFraction = 0;
        readMode = null;
        writeMode = null;
        cachedCells = 0;
        cachedBytes = 0;
        phase = "downloading";
        if (out.kind === "device") out.ctx.phase("downloading", l.totalBytes);

        const plan = planCells(resolution, store.catalog, indices, store.terrain);
        const cells: WorkerCell[] = [];
        const sourceCells: WorkerSourceCell[] = [];
        const terrainCells: WorkerTerrainCell[] = [];
        abortCtl = new AbortController();

        // Cells go to disk when this browser will take them, which keeps a country's
        // worth out of the tab's heap. With the opt-in above, the same store also lets
        // a later build reuse them. The raster deliberately does not:
        // its objects are small and it is downloaded last, so it would buy the
        // least of anything here (a B-series follow-up if that stops being true).
        // Probed by writing and reading a file back, not by sniffing for a
        // method: the fallback has to be chosen on what this browser actually
        // does, and a store that cannot be written to is worse than no store.
        const revision = cellStoreRevision(store.catalog);
        // Unchecked means no reuse in either direction. OPFS still carries this run when available,
        // because that working disk is what makes country-sized assembly possible.
        if (!keepCells) await clearCellStores();
        let cellStore = (await cellStoreWritable()) ? await openCellStore(revision) : null;
        transientCellRevision = cellStore && !keepCells ? revision : null;
        let fetchPlan = plan;
        if (cellStore) {
            fetchPlan = await skipCached(plan, cellStore);
            // Asked once, before a byte is fetched, and asked about the WHOLE
            // run — cells, the map the sink writes, the merge's spill — because
            // after phase D all three live in OPFS: a store with room for the
            // cells but not the output would fail at the first write, after the
            // download. Falling back now costs disk-backed input and any selected
            // reuse, but avoids a quota failure after the download.
            if (!(await hasRoomFor(runDiskNeed(l))))  {
                cellStore = null;
                fetchPlan = plan;
                cachedCells = 0;
                cachedBytes = 0;
            }
        }

        try {
            await downloadCells(fetchPlan, {
                fetchImpl: store.client.fetchImpl,
                onCell: async (item, bytes) => {
                    // `band === null` is what a terrain cell is (`OBCC_Spec.md`
                    // §13: a second artifact class, not a band), and it is the
                    // only thing that decides which door it goes in.
                    if (item.band === null) {
                        terrainCells.push({ id: item.cell.id, sha256: item.cell.sha256, bytes });
                    } else if (cellStore) {
                        // Verified once, on the way in: the file's name is the
                        // digest `fetchVerified` just checked. Awaited, so a slow
                        // disk applies backpressure instead of letting the
                        // download queue gigabytes behind it.
                        await cellStore.put(item.cell.sha256, bytes).catch((cause: unknown) => {
                            throw new Error(
                                `The map could not be saved to this browser's storage (${
                                    cause instanceof Error ? cause.message : String(cause)
                                }). Free some disk space and try again.`,
                            );
                        });
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
                    dlProgress = p;
                    if (out.kind === "device") out.ctx.progress(p.receivedBytes, p.totalBytes);
                },
                signal: abortCtl.signal,
            });
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
            await failRun(e);
            return;
        }

        phase = "assembling";
        if (out.kind === "device") out.ctx.phase("assembling", 0);
        asmPhase = "open";
        const req: AssembleWorkerRequest = {
            type: "assemble",
            cells,
            // One or the other, never a mix: `cells` carries buffers when there
            // was nowhere to put them, `sourceCells` names files in OPFS when
            // there was. The worker decides how it reads the latter — through
            // sync access handles if it has them (#1116 B2), by reading them
            // back into memory if not.
            sourceCells: cellStore ? sourceCells : undefined,
            cellStore: cellStore?.revision,
            knownEmpty: plan.knownEmpty,
            // Terrain travels as its own pair: the lattice the catalog states
            // and the objects that were downloaded. A catalog with no terrain
            // block sends neither, and the map is written with an empty §1.3
            // region — a complete map with flat profiles (§13).
            terrain: store.terrain
                ? { postingLog2: store.terrain.posting_log2, cellLog2: store.terrain.cell_log2 }
                : undefined,
            terrainCells: store.terrain ? terrainCells : undefined,
            schemaJson: store.rootBody,
            skinJson: JSON.stringify(store.skin),
            options: {
                // The same budget the estimate was given, so the projection prices
                // the run that actually happens rather than the engine's default.
                mergeBudgetBytes: SORT_BUDGET_BYTES,
                // Both were shown before this button unlocked. `acceptHoles`
                // is derived from the *shown* set, not the ledger's raw count
                // (#1041 A5): `store.holeCells()` is every band's holes — the
                // squares hatched on the map, the proof and the summary line —
                // so the assembly is never told to accept a hole the UI kept
                // to itself. Partial cells exist in essentially every real map
                // (every coarse cell of a country is partial, #1025).
                acceptHoles: store.holeCells().length > 0,
                acceptPartial: true,
            },
        };
        ensureWorker().postMessage(req, { transfer: requestTransferList(req) });
    }

    function run() {
        void begin({ kind: "download" }).catch((cause) => failRun(cause));
    }

    /** Assemble this selection and stream its verified `.obcm` directly into
     * the connected device's v4 flat-store PUT. The Blob is OPFS-backed when
     * this host passed the writable-storage probe, and memory-priced otherwise. */
    export const sendToDevice: SendAssembledMap = (client, ctx) =>
        new Promise<PutResponse>((resolve, reject) => {
            const onAbort = () => void failRun(ctx.signal.reason ?? new DOMException("cancelled", "AbortError"));
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
            };
            ctx.signal.addEventListener("abort", onAbort, { once: true });
            if (ctx.signal.aborted) {
                device.removeAbort();
                reject(ctx.signal.reason);
                return;
            }
            void begin(device).catch((cause) => {
                if (output === device) void failRun(cause);
                else {
                    device.removeAbort();
                    reject(cause);
                }
            });
        });

    function cancel() {
        const cause = new DOMException("cancelled", "AbortError");
        // Direct assembly and PUT are one DeviceJob. Ask its controller to
        // cancel so both this button and TransferBar cancel the same signal;
        // merely aborting the cell downloader would let a live PUT commit and
        // only then paint the run as cancelled.
        if (output?.kind === "device") {
            output.ctx.cancel(cause);
            return;
        }
        if (phase === "downloading") {
            abortCtl?.abort(cause);
        } else if (phase === "assembling" || phase === "saving") {
            // The worker is blocked inside one synchronous wasm call and cannot
            // read a message — terminate IS the cancel (bridge threading
            // contract). Nothing usable is left behind: a partial `.obcm` fails
            // its own header checks, and the file is only saved once the run says
            // it finished.
            void failRun(cause);
        }
    }

    // --- the memory projection, before the download -----------------------

    let estimate = $state<MemoryEstimate | null>(null);
    let estimatePending = $state(false);
    /** The estimate's own failure channel (#1041 A3) — never mixed into the
     *  run's. Cleared by the next request; retried by bumping the nonce. */
    let estimateError = $state<string | null>(null);
    let estimateNonce = $state(0);

    /** A phone's tab gets nowhere near a desktop's 3 GiB before the browser
     *  evicts the page — and eviction loses the download with no error to
     *  show. 1 GiB is the bridge test's own phone-shaped judgement. */
    const isMobileUa = typeof navigator !== "undefined" && /Android|iPhone|iPad|Mobile/i.test(navigator.userAgent);
    const MOBILE_BUDGET = 1024 ** 3;

    $effect(() => {
        void estimateNonce; // the retry button's lever
        const l = ledger;
        const idle = phase === "idle" || phase === "done" || phase === "cancelled" || phase === "error";
        if (!l || !l.isFinal || l.cellCount === 0 || !idle) {
            // Every exit clears the pending flag (#1041 A3): a selection that
            // empties or a run that starts must not leave "waiting for an
            // estimate" latched with nothing left to answer it.
            estimate = null;
            estimatePending = false;
            return;
        }
        const networkBandBytes = l.core.bytes;
        const totalCellBytes = l.totalBytes;
        const terrainBytes = l.terrain?.bytes ?? 0;
        const diskNeed = runDiskNeed(l);
        estimatePending = true;
        estimateError = null;
        // Debounced: a slider mid-drag changes the figures every frame, and the
        // projection only matters once the selection settles.
        const timer = setTimeout(() => {
            // The main thread's half of both residency escapes, decided by the
            // same two checks `begin` runs before a byte is fetched: a store this
            // browser will write, with room for the WHOLE run (`runDiskNeed` —
            // cells, output, spill; terrain never goes to disk). The worker ANDs
            // in its own sync-handle probe. Responses arrive in request order, so
            // a stale answer is overwritten, never kept.
            void (async () => {
                const onDisk = (await cellStoreWritable()) && (await hasRoomFor(diskNeed));
                ensureWorker().postMessage({
                    type: "estimate",
                    networkBandBytes,
                    totalCellBytes,
                    terrainBytes,
                    onDisk,
                    mergeBudgetBytes: SORT_BUDGET_BYTES,
                    budgetBytes: isMobileUa ? MOBILE_BUDGET : undefined,
                } satisfies AssembleWorkerRequest);
            })();
        }, 500);
        return () => clearTimeout(timer);
    });

    const memoryRefusal = $derived.by(() => {
        if (!estimate || estimate.fits) return null;
        return (
            `Assembling this selection needs about ${formatBytes(estimate.peakBytes)} of browser memory — more than ` +
            `${isMobileUa ? "a phone's tab" : "a browser tab"} can be trusted with ` +
            // A card carries **one** map, because the device has no way to choose
            // between two. So "split it in two" is not a remedy that exists here,
            // and the only honest instruction is to cover less ground.
            `(${formatBytes(estimate.budgetBytes)}). Reduce the coverage area.`
        );
    });

    const memoryCaution = $derived.by(() => {
        if (!estimate || !estimate.fits) return null;
        if (estimate.headroomBytes >= estimate.budgetBytes * 0.15) return null;
        return (
            `Close to the browser's memory budget: about ${formatBytes(estimate.peakBytes)} projected of ` +
            // No "use the desktop app" here: the desktop host runs this very
            // worker in its webview, so it carries the identical wasm32 ceiling.
            // It becomes the sure path when it assembles through the native
            // engine, and this line can promise it then — not before.
            `${formatBytes(estimate.budgetBytes)}. It will probably assemble; a smaller area is the sure thing.`
        );
    });

    // --- the coverage proof -----------------------------------------------

    interface Proof {
        viewW: number;
        viewH: number;
        coverage: string;
        holes: { x: number; y: number; w: number; h: number }[];
        routes: string[];
        holeCount: number;
        gapCount: number;
    }

    const proof = $derived.by<Proof | null>(() => {
        const resolution = store.resolution;
        const l = ledger;
        if (!resolution || !l || l.cellCount === 0) return null;
        const cellIds = resolution.cellsByBand.get(detailBand) ?? [];
        if (cellIds.length === 0) return null;
        const cells = parseCells(cellIds);
        const rings = coverageRings(cells);
        // Every band's holes, the same squares the map hatches (#1041 A5 —
        // `store.holeCells` owns the dedup and the reasoning).
        const holeCells = store.holeCells();
        const holeRects: UBox[] = mergeMixedCellRects(holeCells);
        const routes = store.selection.parts.flatMap((p) => (p.kind === "corridor" ? [p.points] : []));

        // Frame everything in an equirectangular plane, east-west corrected at
        // the middle latitude so shapes keep their look.
        let minLat = Infinity;
        let maxLat = -Infinity;
        let minLon = Infinity;
        let maxLon = -Infinity;
        const stretch = (lat: number, lon: number) => {
            if (lat < minLat) minLat = lat;
            if (lat > maxLat) maxLat = lat;
            if (lon < minLon) minLon = lon;
            if (lon > maxLon) maxLon = lon;
        };
        for (const ring of rings) for (const [lat, lon] of ring) stretch(lat, lon);
        for (const r of holeRects) {
            stretch(r.minLat, r.minLon);
            stretch(r.maxLat, r.maxLon);
        }
        for (const route of routes) for (const p of route) stretch(p.lat, p.lon);
        if (!Number.isFinite(minLat)) return null;

        const viewW = 330;
        const viewH = 200;
        const margin = 10;
        const kx = Math.cos((((minLat + maxLat) / 2) * Math.PI) / 180e6);
        const spanX = Math.max(1, (maxLon - minLon) * kx);
        const spanY = Math.max(1, maxLat - minLat);
        const scale = Math.min((viewW - 2 * margin) / spanX, (viewH - 2 * margin) / spanY);
        const ox = (viewW - spanX * scale) / 2;
        const oy = (viewH - spanY * scale) / 2;
        const px = (lon: number) => ox + (lon - minLon) * kx * scale;
        const py = (lat: number) => oy + (maxLat - lat) * scale;

        const ringPath = (ring: RingPoint[]) =>
            ring.map(([lat, lon], k) => `${k === 0 ? "M" : "L"}${px(lon).toFixed(1)} ${py(lat).toFixed(1)}`).join("") +
            "Z";

        return {
            viewW,
            viewH,
            coverage: rings.map(ringPath).join(""),
            holes: holeRects.map((r) => ({
                x: px(r.minLon),
                y: py(r.maxLat),
                w: (r.maxLon - r.minLon) * kx * scale,
                h: (r.maxLat - r.minLat) * scale,
            })),
            routes: routes.map(
                (route) =>
                    route
                        .map((p, k) => `${k === 0 ? "M" : "L"}${px(p.lon).toFixed(1)} ${py(p.lat).toFixed(1)}`)
                        .join(""),
            ),
            holeCount: holeCells.length,
            gapCount: Math.max(0, patchCount(cells) - 1),
        };
    });

    function proofCaption(p: Proof): string {
        const bits: string[] = [];
        if (p.holeCount) bits.push(`${p.holeCount} ${p.holeCount === 1 ? "hole" : "holes"}`);
        // Disjoint parts are a choice, not a defect (#1041 low sweep / A21):
        // someone who added Freiburg and Bremen knows they are apart, and "1
        // gap" tallied next to the holes read as a warning about it. The
        // mock's own tone — "gaps are fine — holes stay visible" — is the
        // caption's tone: reassure about the gaps, stay honest about holes.
        if (p.gapCount) bits.push("separate parts — that's fine");
        return bits.length ? `your map's coverage — ${bits.join(" · ")}` : "your map's coverage";
    }

    // --- gating -----------------------------------------------------------

    const running = $derived(phase === "downloading" || phase === "assembling" || phase === "saving");
    const refusal = $derived.by(() => {
        const l = ledger;
        if (!l || l.cellCount === 0) return null;
        if (l.verdict.kind === "refuse") return l.verdict.message;
        return memoryRefusal;
    });
    const ready = $derived.by(() => {
        const l = ledger;
        return (
            l !== null &&
            l.isFinal &&
            l.cellCount > 0 &&
            refusal === null &&
            !running &&
            !estimatePending &&
            // An unanswered projection keeps the mandatory pre-download check
            // honest: the button waits for the retry, not forever (A3).
            estimateError === null
        );
    });
    $effect(() => onSendReadyChange?.(ready));
    /** Whether a failed run left a file behind that someone has to delete. Counted
     *  from what actually reached the disk, and only where nothing cleaned it up:
     *  a picked directory's session removes what it wrote, so there is something to
     *  discard only when there was no session or its removal refused. */
    const incompleteFileRemains = $derived(
        persisted && (!platform.openMapOutput || outputCleanupFailed),
    );

    const dlPct = $derived(
        dlProgress && dlProgress.totalBytes > 0
            ? Math.min(100, Math.round((dlProgress.receivedBytes / dlProgress.totalBytes) * 100))
            : 0,
    );
</script>

{#if !ledger || ledger.cellCount === 0}
    <p class="line muted small">Nothing to download yet — add coverage in step 1.</p>
{:else}
    <div class="split">
        {#if proof}
            <figure class="proof">
                <svg viewBox="0 0 {proof.viewW} {proof.viewH}" role="img" aria-label={proofCaption(proof)}>
                    <defs>
                        <pattern id="proof-hatch" width="8" height="8" patternUnits="userSpaceOnUse">
                            <path d="M0 8 L8 0" stroke="var(--coral)" stroke-width="1.4" />
                        </pattern>
                    </defs>
                    <path
                        d={proof.coverage}
                        fill="var(--amber)"
                        fill-opacity="0.14"
                        fill-rule="evenodd"
                        stroke="var(--amber)"
                        stroke-width="1.6"
                    />
                    {#each proof.holes as r, k (k)}
                        <rect x={r.x} y={r.y} width={r.w} height={r.h} fill="url(#proof-hatch)" opacity="0.55" />
                        <rect
                            x={r.x}
                            y={r.y}
                            width={r.w}
                            height={r.h}
                            fill="none"
                            stroke="var(--coral)"
                            stroke-width="1"
                        />
                    {/each}
                    {#each proof.routes as d, k (k)}
                        <path {d} fill="none" stroke="var(--coral)" stroke-width="1.6" />
                    {/each}
                </svg>
                <figcaption class="small faint">{proofCaption(proof)}</figcaption>
            </figure>
        {/if}

        <div class="facts">
            <!-- P1 (2026-08-09): the one number that matters leads the card as
                 a stat band; everything descriptive is one quiet caption. -->
            {#if ledger.isFinal}
                <p class="line statband">
                    <span class="mono big">{formatBytes(ledger.totalBytes)}</span>
                    <span class="small faint">
                        {ledger.cellCount}
                        {ledger.cellCount === 1 ? "cell" : "cells"} · assembled on this computer
                    </span>
                </p>
            {:else}
                <p class="line statband"><span class="mono big faint">pricing…</span></p>
            {/if}
            <p class="line small faint">
                <span class="serif">{store.catalog.schema.name}</span> schema ·
                <span class="serif">{store.skin.name}</span> skin
            </p>

            {#if refusal}
                <p class="line warn small">{refusal}</p>
            {:else if estimateError}
                <p class="line warn small">
                    Couldn't project the memory this assembly needs — the check runs before any
                    download: {estimateError}
                    <button type="button" class="retry" onclick={() => (estimateNonce += 1)}
                        >retry</button
                    >
                </p>
            {:else if memoryCaution}
                <p class="line caution small">{memoryCaution}</p>
            {/if}

            <div class="cell-storage small">
                <label>
                    <input
                        type="checkbox"
                        checked={keepCells}
                        disabled={running || clearingCells}
                        onchange={(event) => void setKeepCells(event.currentTarget.checked)}
                    />
                    <span>
                        Keep downloaded map cells for future builds
                        <span class="faint">— otherwise they are temporary and deleted after this build.</span>
                    </span>
                </label>
                <button
                    type="button"
                    class="clear-cells"
                    disabled={running || clearingCells}
                    onclick={() => void clearStoredCells(true)}
                >
                    {clearingCells ? "Deleting…" : "Delete stored map data"}
                </button>
                {#if cellStorageNotice}<span class="faint">{cellStorageNotice}</span>{/if}
            </div>

            {#if phase === "idle" || phase === "cancelled" || phase === "error" || phase === "done"}
                <!-- No size in the label: the stat band above already leads
                     with it, and the same number twice in one card was the P1
                     round's headline complaint. -->
                <button type="button" class="btn primary" disabled={!ready} onclick={run}>
                    Download map
                </button>
            {:else}
                <div class="runrow">
                    <button type="button" class="btn" onclick={cancel}>Cancel</button>
                    {#if phase === "downloading" && dlProgress}
                        <span class="small muted">
                            downloading cells — {dlProgress.completedCells}/{dlProgress.totalCells} ·
                            {formatBytes(dlProgress.receivedBytes)} of {formatBytes(dlProgress.totalBytes)}
                            {#if cachedCells > 0}· {cachedCells} already on this computer ({formatBytes(
                                    cachedBytes,
                                )}){/if}
                        </span>
                    {:else if phase === "assembling"}
                        <span class="small muted">
                            assembling — {ASM_PHASE_LABEL[asmPhase]} · {Math.round(asmFraction * 100)}%{#if readMode}
                                · cells {readMode}{/if}
                        </span>
                    {:else if phase === "saving"}
                        <span class="small muted">
                            {output?.kind === "device" ? "sending the map" : "saving the map"}{#if runFileName}
                                — {runFileName}{/if}
                        </span>
                    {/if}
                </div>
                <div class="bar">
                    <span
                        style:width={`${phase === "downloading" ? dlPct : Math.round(asmFraction * 100)}%`}
                        class:assembling={phase === "assembling" || phase === "saving"}
                    ></span>
                </div>
            {/if}

            {#if phase === "done"}
                <div class="done">
                    {#if lastRunKind === "device"}
                        <p class="line small">Assembled and sent <span class="mono">{runFileName}</span>.</p>
                    {:else}
                        <p class="line small">
                            {#if outputPath}
                                Saved <span class="mono">{savedFile?.name}</span> in
                                <span class="mono">{outputPath}</span>.
                            {:else}
                                Downloaded <span class="mono">{savedFile?.name}</span>.
                            {/if}
                        </p>
                    {/if}
                    {#if savedFile}
                        <p class="line faint small mono">{formatBytes(savedFile.byteLength)}</p>
                    {/if}
                    {#if readMode}
                        <p class="line faint small mono">
                            cells {readMode}{#if writeMode}
                                · map {writeMode}{/if}{#if cachedCells > 0}
                                · {cachedCells} reused from this computer ({formatBytes(cachedBytes)} not
                                downloaded){/if}
                        </p>
                    {/if}
                    {#each runWarnings as w, k (k)}
                        <p class="line caution small">{w}</p>
                    {/each}
                </div>
            {:else if phase === "error"}
                <p class="line warn small">
                    {#if incompleteFileRemains}
                        The map was not completed. Discard the file it left behind: {errorMessage}
                    {:else}
                        Nothing was saved: {errorMessage}
                    {/if}
                </p>
            {:else if phase === "cancelled"}
                <p class="line faint small">
                    {#if incompleteFileRemains}
                        Cancelled — discard the incomplete file it left behind.
                    {:else}
                        Cancelled — nothing was saved.
                    {/if}
                </p>
            {:else if phase === "idle"}
                <p class="line faint small">
                    Cells are verified against the catalog (SHA-256), then assembled and read back in full
                    before anything reaches the card.
                </p>
            {/if}
        </div>
    </div>
{/if}

<style>
    .split {
        display: flex;
        gap: 14px;
        align-items: flex-start;
        flex-wrap: wrap;
    }

    .proof {
        margin: 0;
        flex: 0 1 240px;
        min-width: 180px;
    }

    .proof svg {
        width: 100%;
        height: auto;
        background: var(--parchment-2);
        border: 1px solid var(--parchment-3);
        border-radius: 8px;
    }

    .proof figcaption {
        text-align: center;
        margin-top: 4px;
    }

    .facts {
        flex: 1 1 240px;
        min-width: 220px;
        display: flex;
        flex-direction: column;
        gap: 8px;
    }

    .line {
        margin: 0;
        line-height: 1.45;
    }

    .statband {
        display: flex;
        align-items: baseline;
        gap: 10px;
        flex-wrap: wrap;
    }

    .statband .big {
        font-size: 19px;
        font-weight: 600;
    }

    .serif {
        font-family: var(--serif);
        font-style: italic;
    }

    .line.warn {
        color: var(--coral);
    }

    .retry {
        background: none;
        border: none;
        color: var(--forest);
        text-decoration: underline;
        padding: 0;
        font-size: inherit;
    }

    .line.caution {
        background: rgba(227, 173, 51, 0.18);
        border: 1px solid var(--amber);
        border-radius: 8px;
        padding: 6px 9px;
    }

    .btn {
        align-self: flex-start;
    }

    .cell-storage {
        display: flex;
        flex-direction: column;
        align-items: flex-start;
        gap: 4px;
        padding: 7px 9px;
        border: 1px solid var(--parchment-3);
        border-radius: 8px;
    }

    .cell-storage label {
        display: flex;
        align-items: flex-start;
        gap: 7px;
        cursor: pointer;
    }

    .cell-storage input {
        margin-top: 3px;
    }

    .clear-cells {
        border: 0;
        padding: 0;
        background: none;
        color: var(--forest);
        text-decoration: underline;
        font: inherit;
        cursor: pointer;
    }

    .clear-cells:disabled,
    .cell-storage label:has(input:disabled) {
        cursor: default;
        opacity: 0.6;
    }

    .runrow {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .bar {
        height: 6px;
        border-radius: 999px;
        background: var(--parchment-2);
        overflow: hidden;
    }

    .bar span {
        display: block;
        height: 100%;
        background: var(--forest);
        transition: width 0.2s;
    }

    @media (prefers-reduced-motion: reduce) {
        .bar span {
            transition: none;
        }
    }

    .bar span.assembling {
        background: var(--wood);
    }

    .done {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
</style>
