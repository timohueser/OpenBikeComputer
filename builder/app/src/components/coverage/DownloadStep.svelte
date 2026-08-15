<script lang="ts">
    // Final coverage proof and verified download/assembly. File and wasm-memory
    // limits are checked before transfer. Synchronous wasm runs in a Worker;
    // files return one at a time for browser, desktop, or device output.

    import { onDestroy, onMount } from "svelte";
    import type { AssemblePhase, MemoryEstimate } from "../../lib/assemble/bridge";
    import {
        isWorkerResponse,
        requestTransferList,
        type AssembleWorkerRequest,
        type CellReadMode,
        type ShardWriteMode,
        type WorkerCell,
        type WorkerFile,
        type WorkerSourceCell,
        type WorkerStoredShard,
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
        readShardOutput,
        type CellStore,
    } from "../../lib/cells/store";
    import { saveBlob } from "../../lib/download";
    import { storeZip } from "../../lib/zip";
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
    import { formatBytes, truncateUtf8 } from "../../lib/format";
    import type { JobContext } from "../../lib/device/progress";
    import type { SetSendState } from "../../lib/device/write";
    import type { ProtocolClient, UploadResult } from "../../lib/usb/client";

    let { store }: { store: CoverageStore } = $props();

    const KEEP_CELLS_KEY = "obcm.keepMapCells";

    function loadKeepCells(): boolean {
        try {
            return globalThis.localStorage?.getItem(KEEP_CELLS_KEY) === "true";
        } catch {
            return false;
        }
    }

    /** Where a geometry shard is split (OBCA §5). 256 MB, against the engine's
     *  1 GiB default: it is the largest piece this screen is willing to have
     *  resident in wasm memory at once, and the smallest that does not turn a
     *  country into dozens of files (§5's ceiling is 32 shards per set). */
    const TARGET_SHARD_BYTES = 256 * 1024 * 1024;

    /** The engine's sort budget (#1116 phase D), which after the external merge **is** the wasm
     *  engine term. 256 MB: big enough that a DACH-scale sort is ~a dozen runs (well inside the
     *  scratch pool), small enough that engine + caches + terrain stays a fraction of a tab's
     *  budget. Passed to both the assembly and the estimate, so the projection prices the run
     *  that will actually happen. */
    const SORT_BUDGET_BYTES = 256 * 1024 * 1024;

    /** What a run of this ledger needs from OPFS, all three tenants together: the cells (minus
     *  terrain, which never goes to disk), the assembled set the sink writes (≈ the cells, measured
     *  1.00), and the merge's spill (edge + adjacency + claim streams; ≤ 2.5× the network band,
     *  the estimate model's own coefficient). Used by the estimate effect and by `begin`, so the
     *  projection and the run decide OPFS-or-fallback by the same arithmetic — a run that can
     *  store its cells but not its shards would otherwise fail at shard one, after the download. */
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
        if (output?.kind === "device") {
            void failRun(new DOMException("The builder was closed.", "AbortError"));
        } else {
            worker?.terminate();
            void closeDownloadOutput(true);
            void cleanupTransientCells();
        }
    });

    // Releases before the opt-in retained cells automatically. The first visit to this step after
    // the change removes that legacy cache unless the rider has explicitly enabled reuse.
    onMount(() => {
        if (!keepCells) void clearStoredCells(false);
    });

    // --- the run ----------------------------------------------------------

    type Phase = "idle" | "downloading" | "assembling" | "done" | "cancelled" | "error";
    let phase = $state<Phase>("idle");
    let dlProgress = $state<CellDownloadProgress | null>(null);
    let asmPhase = $state<AssemblePhase>("open");
    let asmFraction = $state(0);
    let savedFiles = $state<{ name: string; role: string; byteLength: number; path?: string }[]>([]);
    /** Files actually on disk. Not `savedFiles.length`: a browser host *stages*
     *  everything and only saves at the end, so a run that failed halfway left
     *  the list populated and the disk untouched — and "discard the 3 files you
     *  downloaded" about files nobody downloaded is the wrong instruction. */
    let persistedFiles = $state(0);
    /** The no-picker browser host's staging area (#1116 B1). Shards arrive
     *  *during* the assembly, and saving each one as it lands would drop half a
     *  map into someone's Downloads folder the moment they pressed cancel. They
     *  are held as Blobs — the big ones are OPFS-backed Files, so no heap is
     *  pinned — and delivered together once the set is complete, as ONE zip
     *  download: a save per file is how Firefox ends up with a stack of dialogs
     *  and orphaned `.part` files. */
    let staged: { name: string; blob: Blob }[] = [];
    /** True while the staged set is being packaged into its archive — the CRC
     *  pass reads every staged byte once, which at country scale is seconds. */
    let packaging = $state(false);
    let packFraction = $state(0);
    /** How the finished set reached the disk, for the done message: files in a
     *  picked folder, or one archive in the downloads. */
    let deliveredAs = $state<"folder" | "zip" | null>(null);
    /** Files arrive with no acknowledgement now, so two can be in flight at
     *  once; every sink write chains onto this instead of racing. */
    let sink: Promise<void> = Promise.resolve();
    /** Set by `failRun`: whatever is still queued must not be written into an
     *  output that is being discarded. */
    let sinkClosed = false;
    /** How this run's cells reached the assembler (#1116 B2), as the worker
     *  reports it before the assembly starts. Shown, because a bug report about a
     *  failed country-scale run has to say which path ran. */
    let readMode = $state<CellReadMode | null>(null);
    /** …and where its shards went (#1116 D1), for the same reason. */
    let writeMode = $state<ShardWriteMode | null>(null);
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
    let runMapName = "OBC map";

    interface DeviceOutput {
        kind: "device";
        client: ProtocolClient;
        ctx: JobContext;
        state: SetSendState | null;
        resolve: (result: UploadResult) => void;
        reject: (cause: unknown) => void;
        settled: boolean;
        failing: boolean;
        removeAbort: () => void;
    }
    type Output = { kind: "download" } | DeviceOutput;
    let output = $state<Output | null>(null);
    let lastRunKind = $state<Output["kind"]>("download");

    const ASM_PHASE_LABEL: Record<AssemblePhase, string> = {
        open: "reading cells",
        poi: "merging places",
        nav: "stitching the road network",
        plan: "planning files",
        write: "writing",
        verify: "checking the result",
        manifest: "sealing the set",
        done: "done",
    };

    const ROLE_LABEL: Record<string, string> = {
        core: "routing + places",
        coarse: "zoomed-out overview",
        geometry: "map detail",
        terrain: "elevation",
        manifest: "set manifest",
    };

    async function onWorkerMessage(e: MessageEvent) {
        const msg = e.data as unknown;
        if (!isWorkerResponse(msg)) return;
        switch (msg.type) {
            case "estimate-result":
                estimate = msg.estimate;
                deviceEstimate = msg.deviceEstimate;
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
            case "planned":
                runWarnings = msg.warnings;
                if (output?.kind === "device") {
                    output.state = {
                        shardCount: msg.shardCount,
                        totalBytes: msg.totalBytes,
                        committedBytes: 0,
                        nextShard: 0,
                        terrainSent: false,
                        setId: null,
                    };
                    output.ctx.phase("sending", msg.totalBytes);
                }
                break;
            case "shard":
                // Evicted from wasm memory mid-run and posted with no ack — the
                // assembly cannot wait for one (worker protocol header). So the
                // write is queued rather than awaited here, and its failure has
                // nobody to return to: it fails the run itself.
                void queueSave(() => saveAssembledFile(msg)).catch((cause) => failRun(cause));
                break;
            case "stored-shard":
                // The shard never crossed the port: it is in OPFS, the worker has
                // let go of it, and this is where it becomes a file someone has.
                // Acknowledged like a `file`, so the next one waits.
                try {
                    await queueSave(() => saveStoredShard(msg));
                    worker?.postMessage({ type: "file-ack" } satisfies AssembleWorkerRequest);
                } catch (cause) {
                    await failRun(cause);
                }
                break;
            case "file":
                try {
                    if (output?.kind === "device") {
                        if (!output.state) throw new Error("The assembler sent a file before its set plan.");
                        const { sendAssembledSetFile } = await import("../../lib/device/write");
                        await sendAssembledSetFile(output.client, output.state, msg, output.ctx);
                        savedFiles.push({ name: msg.name, role: msg.role, byteLength: msg.byteLength });
                    } else {
                        // The worker waits for this ack, so the next file never
                        // competes for memory or arrives before this write ends.
                        await queueSave(() => saveAssembledFile(msg));
                    }
                    worker?.postMessage({ type: "file-ack" } satisfies AssembleWorkerRequest);
                } catch (cause) {
                    await failRun(cause);
                }
                break;
            case "done":
                // The worker's OPFS ledger, for anyone profiling an assembly
                // from DevTools — a worker's own console does not surface.
                if (msg.io) console.debug("[assemble] opfs i/o", msg.io);
                runWarnings = msg.warnings;
                if (output?.kind === "device") {
                    phase = "done";
                    const state = output.state;
                    if (!state || state.setId === null) {
                        await failRun(new Error("The device did not commit the assembled map's manifest."));
                    } else {
                        settleDevice({ objectId: state.setId, committedOffset: state.totalBytes });
                    }
                } else {
                    try {
                        // Streamed shards were queued, not awaited: drain before
                        // calling the map finished.
                        await sink;
                        // …and only now, with a complete set, does the browser
                        // host actually hand anything to the downloader.
                        await packageStaged();
                        await closeDownloadOutput(false);
                    } catch (cause) {
                        await failRun(cause);
                        break;
                    }
                    phase = "done";
                }
                await cleanupTransientCells();
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
                if (phase === "assembling") {
                    await failRun(new Error(msg.message));
                } else {
                    estimateError = msg.message;
                }
                estimatePending = false;
                break;
        }
    }

    /**
     * Put one finished file where this host keeps them, one at a time.
     *
     * Serialized through `sink` because streamed shards arrive unacknowledged:
     * two `onmessage` handlers can be in flight at once, and `openMapOutput` is
     * a lazily-opened folder that must not be opened twice.
     */
    function queueSave(save: () => Promise<void>): Promise<void> {
        const done = sink.then(save);
        // The chain itself must survive a failure — `failRun` closes the sink,
        // so what follows is a no-op rather than a write into a discarded
        // output — while the caller still sees the rejection.
        sink = done.catch(() => {});
        return done;
    }

    async function saveAssembledFile(file: WorkerFile) {
        await savePart(file, () => new Blob([file.bytes as unknown as BlobPart]), file.bytes);
    }

    /**
     * One shard the assembly wrote into OPFS (#1116 D1): its bytes were never in
     * wasm memory and never crossed the worker port, and on the browser host they
     * never enter the tab's heap either — a `Blob` on an OPFS file is a handle, and
     * that is what the downloader is given.
     *
     * The desktop host does read it back, because its output session takes bytes to
     * write into a real folder. That is one shard resident, which is exactly what it
     * was before this existed — the saving D1 is here for is on the *assembly*, and
     * it is unaffected.
     */
    async function saveStoredShard(shard: WorkerStoredShard) {
        const blob = await readShardOutput(shard.entry);
        if (blob.size !== shard.byteLength) {
            throw new Error(
                `The assembled shard ${shard.name} is ${blob.size} bytes in this browser's storage, not the ` +
                    `${shard.byteLength} the assembler wrote. Free some disk space and try again.`,
            );
        }
        await savePart(shard, () => blob, null);
    }

    /** The two hosts' save, shared: a Blob for the browser, bytes for the desktop. */
    async function savePart(
        part: { name: string; role: string; byteLength: number },
        blob: () => Blob,
        bytes: Uint8Array | null,
    ) {
        if (sinkClosed) return;
        if (downloadOutput) {
            // A grouped output (desktop folder, or the picked directory of a
            // browser with the File System Access API) takes every part as it
            // lands. The session streams a Blob without buffering it; only a
            // host that needs contiguous bytes converts, on its side.
            const path = await downloadOutput.write(part.name, bytes ?? blob());
            savedFiles.push({ name: part.name, role: part.role, byteLength: part.byteLength, path });
            persistedFiles += 1;
        } else {
            // The no-picker browser stages and delivers at the end — see `staged`.
            staged.push({ name: part.name, blob: blob() });
            savedFiles.push({ name: part.name, role: part.role, byteLength: part.byteLength });
        }
    }

    /**
     * The no-picker browser's delivery moment: the set is complete, so it may
     * exist — as **one** download. A save per file is ten simultaneous
     * downloads for a country, which Firefox presents as a stack of modal
     * dialogs whose losers die as `.part` files. One archive is one prompt.
     */
    async function packageStaged() {
        if (staged.length === 0 || sinkClosed) return;
        if (staged.length === 1) {
            // Nothing to bundle — a single file saves as itself.
            saveBlob(staged[0].blob, staged[0].name);
        } else {
            packaging = true;
            packFraction = 0;
            try {
                const archive = await storeZip(
                    staged.map((f) => ({ name: f.name, blob: f.blob })),
                    (done, total) => (packFraction = total > 0 ? done / total : 0),
                );
                // A cancel that raced the CRC pass closed the sink: the archive
                // is dropped, not handed to a downloader nobody asked for.
                if (sinkClosed) return;
                saveBlob(archive, `${zipStem(runMapName)}.zip`);
            } finally {
                packaging = false;
            }
            deliveredAs = "zip";
        }
        persistedFiles += 1;
        staged = [];
    }

    /** The map's name as a filename: what the OS forbids becomes a dash. */
    function zipStem(name: string): string {
        const stem = name.replace(/[\\/:*?"<>|]/g, "-").trim();
        return stem.length > 0 ? stem : "OBC map";
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

    function settleDevice(result: UploadResult | unknown, failed = false) {
        if (output?.kind !== "device" || output.settled) return;
        output.settled = true;
        output.removeAbort();
        if (failed) output.reject(result);
        else output.resolve(result as UploadResult);
    }

    async function failRun(cause: unknown) {
        if (output?.kind === "device") {
            if (output.failing || output.settled) return;
            output.failing = true;
        }
        const cancelled =
            (output?.kind === "device" && output.ctx.signal.aborted) ||
            (cause instanceof DOMException && cause.name === "AbortError");
        abortCtl?.abort();
        worker?.terminate();
        worker = null;
        // Shards handed over mid-run are the caller's to clean up (bridge
        // docs). On the browser host that is simply dropping the staged Blobs —
        // nothing was ever handed to the downloader; on the desktop it is the
        // `discard()` below, which removes the whole output folder. Either way
        // §5.4 already guarantees the device side: the OBCS manifest is written
        // last, so a half-written set is not a map.
        sinkClosed = true;
        staged = [];
        // Everything still queued is now a no-op, but one write may already be
        // in flight — let it land before the output is discarded, or the
        // discard races a file being written into the folder it removes. It is
        // bounded by a single shard's write.
        await sink.catch(() => {});
        if (output?.kind === "device" && output.state) {
            const { abandonAssembledSet } = await import("../../lib/device/write");
            await abandonAssembledSet(output.client, output.state);
        } else {
            await closeDownloadOutput(true).catch(() => (outputCleanupFailed = true));
        }
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

    function mapName(): string {
        const parts = store.selection.parts;
        const base = parts.length === 0 ? "OBC map" : parts[0].name;
        const name = parts.length > 1 ? `${base} +${parts.length - 1}` : base;
        return truncateUtf8(name, 24);
    }

    async function begin(out: Output) {
        const resolution = store.resolution;
        const indices = store.indices;
        const l = ledger;
        if (
            !resolution ||
            !indices ||
            !l ||
            phase === "downloading" ||
            phase === "assembling" ||
            (out.kind === "device" && !ready)
        ) {
            throw new Error("This map is not ready to assemble yet.");
        }
        // A grouped output opens under the click that started the run: the
        // browser's implementation is a directory picker, and a picker without
        // a fresh user activation is refused (the platform contract). A
        // dismissed picker is "changed my mind" — the run never starts, and
        // the screen stays exactly as it was.
        let grouped: MapOutputSession | null = null;
        if (out.kind === "download" && platform.openMapOutput) {
            try {
                grouped = await platform.openMapOutput(mapName());
            } catch (cause) {
                if (cause instanceof DOMException && cause.name === "AbortError") return;
                throw cause;
            }
        }
        output = out;
        lastRunKind = out.kind;
        errorMessage = null;
        savedFiles = [];
        persistedFiles = 0;
        staged = [];
        packaging = false;
        packFraction = 0;
        deliveredAs = grouped ? "folder" : null;
        sink = Promise.resolve();
        sinkClosed = false;
        outputPath = grouped?.path ?? null;
        downloadOutput = grouped;
        outputCleanupFailed = false;
        runWarnings = [];
        dlProgress = null;
        asmFraction = 0;
        readMode = null;
        writeMode = null;
        cachedCells = 0;
        cachedBytes = 0;
        phase = "downloading";

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
            // run — cells, the sink's output, the merge's spill — because after
            // phase D all three live in OPFS: a store with room for the cells
            // but not the shards would fail at shard one, after the download.
            // Falling back now costs disk-backed input and any selected reuse, but avoids a
            // quota failure after the download.
            if (!(await hasRoomFor(runDiskNeed(l))))  {
                cellStore = null;
                fetchPlan = plan;
                cachedCells = 0;
                cachedBytes = 0;
            }
        }
        if (out.kind === "device") out.ctx.phase("downloading", fetchPlan.totalBytes);

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
        runMapName = mapName();
        asmPhase = "open";
        if (out.kind === "device") out.ctx.phase("assembling", 0);
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
            // block sends neither, and the set is written without a `terrain`
            // role — a complete map with flat profiles (§13).
            terrain: store.terrain
                ? { postingLog2: store.terrain.posting_log2, cellLog2: store.terrain.cell_log2 }
                : undefined,
            terrainCells: store.terrain ? terrainCells : undefined,
            schemaJson: store.rootBody,
            skinJson: JSON.stringify(store.skin),
            // Only the download path takes files mid-run. The device upload is
            // built on the set plan — `planned` gives it the shard count and the
            // byte total it opens the transfer with — and that plan does not
            // exist until the run ends, so a device run keeps every shard in
            // wasm memory and takes them afterwards, exactly as before (#1116).
            streamShards: out.kind === "download",
            // …and better still where the browser allows it: the shards go
            // straight into OPFS, so not even one is in wasm memory (#1116 D1).
            // The core shard cannot be split, so at country scale "one shard
            // resident" is still a multi-gigabyte allocation. The device path
            // opts out with `streamShards` and for the same reason — it needs the
            // bytes in hand to push them over USB.
            shardSink: out.kind === "download",
            options: {
                name: runMapName,
                // Split always, at 256 MB. Both halves are about handing the map
                // on in pieces: a shard leaves wasm memory as soon as it is
                // verified, so the assembly's output residency is one shard
                // rather than the whole set — and a 256 MB piece is a plausible
                // unit to resume a card write or an upload at, where a 1 GiB one
                // (the engine's default) is a quarter of a browser's whole
                // memory budget riding on one uninterrupted write.
                forceSplit: true,
                targetShardBytes: TARGET_SHARD_BYTES,
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

    /** Assemble the current selection and stream its files directly to a connected device. */
    export function sendToDevice(client: ProtocolClient, ctx: JobContext): Promise<UploadResult> {
        return new Promise((resolve, reject) => {
            // The device path keeps the whole set in wasm memory until `planned` (#1116 B1's
            // opt-out), so it can refuse a selection the download button honestly accepts.
            // Checked before a byte is fetched — failing here costs a click; failing at the
            // engine's peak costs the download, the rewrite, and the tab.
            if (deviceEstimate && !deviceEstimate.fits) {
                reject(
                    new Error(
                        `Sending straight to the device holds the whole assembled map in browser memory — about ` +
                            `${formatBytes(deviceEstimate.peakBytes)}, more than a tab can be trusted with ` +
                            `(${formatBytes(deviceEstimate.budgetBytes)}). Download the map and copy it to the ` +
                            `card instead, or reduce the coverage area.`,
                    ),
                );
                return;
            }
            const out: DeviceOutput = {
                kind: "device",
                client,
                ctx,
                state: null,
                resolve,
                reject,
                settled: false,
                failing: false,
                removeAbort: () => {},
            };
            const abort = () => void failRun(ctx.signal.reason ?? new DOMException("cancelled", "AbortError"));
            out.removeAbort = () => ctx.signal.removeEventListener("abort", abort);
            ctx.signal.addEventListener("abort", abort, { once: true });
            void begin(out).catch((cause) => failRun(cause));
        });
    }

    function cancel() {
        if (phase === "downloading") {
            abortCtl?.abort();
        } else if (phase === "assembling") {
            // The worker is blocked inside one synchronous wasm call and cannot
            // read a message — terminate IS the cancel (bridge threading
            // contract). Nothing is half-written: the set manifest goes last.
            void failRun(new DOMException("cancelled", "AbortError"));
        }
    }

    // --- the memory projection, before the download -----------------------

    let estimate = $state<MemoryEstimate | null>(null);
    /** The same selection priced as the device path runs it — set kept until
     *  `planned` (#1116 B1's opt-out) — so it binds earlier than `estimate`.
     *  Gates {@link sendToDevice}; the download button never reads it. */
    let deviceEstimate = $state<MemoryEstimate | null>(null);
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
            deviceEstimate = null;
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
            // The main thread's half of the input mode, decided by the same two
            // checks `begin` runs before a byte is fetched: a store this browser
            // will write, with room for the WHOLE run (`runDiskNeed` — cells,
            // output, spill; terrain never goes to disk). The worker ANDs in its
            // own sync-read probe. Responses arrive in request order, so a stale
            // answer is overwritten, never kept.
            void (async () => {
                const inputOnDisk = (await cellStoreWritable()) && (await hasRoomFor(diskNeed));
                ensureWorker().postMessage({
                    type: "estimate",
                    networkBandBytes,
                    totalCellBytes,
                    terrainBytes,
                    inputOnDisk,
                    streamedShardBytes: TARGET_SHARD_BYTES,
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
            // A card carries **one** map — several files of one volume set, never
            // several maps, because the device has no way to choose between them.
            // So "split it in two" is not a remedy that exists here, and the only
            // honest instruction is to cover less ground.
            `(${formatBytes(estimate.budgetBytes)}). Reduce the coverage area.`
        );
    });

    /** Said here, on the coverage screen, rather than first discovered when a device send
     *  refuses: the selection downloads fine but will not fit a direct device send. */
    const deviceCaution = $derived.by(() => {
        if (!estimate?.fits || !deviceEstimate || deviceEstimate.fits) return null;
        return (
            `This map downloads fine, but sending it straight to a device would need about ` +
            `${formatBytes(deviceEstimate.peakBytes)} of browser memory ` +
            `(${formatBytes(deviceEstimate.budgetBytes)} available) — download it and copy it to the card instead.`
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

    const running = $derived(phase === "downloading" || phase === "assembling");
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
    /** Whether a failed run left files behind that someone has to delete. Counted
     *  from what actually reached the disk, not from what the assembler handed
     *  over: the browser host stages a cancelled run's shards and never saves
     *  them, so there is nothing to discard. */
    const incompleteFilesRemain = $derived(
        lastRunKind === "download" && persistedFiles > 0 && (!platform.openMapOutput || outputCleanupFailed),
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
            {:else if deviceCaution}
                <p class="line caution small">{deviceCaution}</p>
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
                    {#if output?.kind === "download"}
                        <button type="button" class="btn" onclick={cancel}>Cancel</button>
                    {/if}
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
                            {#if packaging}
                                packaging the download — {Math.round(packFraction * 100)}%
                            {:else}
                                assembling — {ASM_PHASE_LABEL[asmPhase]} · {Math.round(asmFraction * 100)}%{#if readMode}
                                    · cells {readMode}{/if}
                            {/if}
                        </span>
                    {/if}
                </div>
                <div class="bar">
                    <span
                        style:width={`${phase === "downloading" ? dlPct : Math.round(asmFraction * 100)}%`}
                        class:assembling={phase === "assembling"}
                    ></span>
                </div>
            {/if}

            {#if phase === "done"}
                <div class="done">
                    {#if lastRunKind === "download"}
                        <p class="line small">
                            {#if outputPath}
                                Saved {savedFiles.length}
                                {savedFiles.length === 1 ? "file" : "files"} in
                                <span class="mono">{outputPath}</span> — if that folder isn't the device's
                                card, copy {savedFiles.length === 1 ? "it" : "them"} to its top level.
                            {:else if deliveredAs === "zip"}
                                Saved the whole set as one archive — unzip it and put the files at the top
                                level of the device's card.
                            {:else}
                                Saved {savedFiles.length}
                                {savedFiles.length === 1 ? "file" : "files"} — copy
                                {savedFiles.length === 1 ? "it" : "all of them"} to the top level of the
                                device's card.
                            {/if}
                        </p>
                    {/if}
                    <ul class="files mono small">
                        {#each savedFiles as f (f.name)}
                            <li>
                                {f.name}
                                <span class="faint">
                                    · {ROLE_LABEL[f.role] ?? f.role} · {formatBytes(f.byteLength)}</span
                                >
                            </li>
                        {/each}
                    </ul>
                    {#if readMode}
                        <p class="line faint small mono">
                            cells {readMode}{#if writeMode}
                                · shards {writeMode}{/if}{#if cachedCells > 0}
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
                    {#if incompleteFilesRemain}
                        The map was not completed. Discard the {persistedFiles} downloaded
                        {persistedFiles === 1 ? "file" : "files"}: {errorMessage}
                    {:else}
                        Nothing was saved: {errorMessage}
                    {/if}
                </p>
            {:else if phase === "cancelled"}
                <p class="line faint small">
                    {#if incompleteFilesRemain}
                        Cancelled — discard the {persistedFiles} incomplete
                        {persistedFiles === 1 ? "file" : "files"} already downloaded.
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

    .files {
        list-style: none;
        margin: 0;
        padding: 0;
    }
</style>
