// Downloaded cells, on disk instead of in memory (#1116 B2).
//
// The builder used to hold every downloaded cell as a `Uint8Array` from the moment
// it arrived until the assembly was over — ~795 MB for Baden-Württemberg, in the
// tab's heap and then copied again into wasm's. And a reload threw all of it away.
// This module is where they go instead: the browser's **origin private file
// system**, keyed by the digest the catalog already pins them with.
//
// Two sides, two APIs, and the split is not ours — it is the platform's:
//
//   * **Writing happens on the main thread**, through `createWritable()`, next to
//     the download that produced the bytes.
//   * **Reading happens in the assembly worker**, through
//     `createSyncAccessHandle()`, which exists *only* in a dedicated worker and is
//     the only synchronous file read a browser has. That is what lets the wasm
//     engine pull a byte range from inside its own synchronous `run()` — see
//     `../assemble/bridge.ts`.
//
// **Sync handles cannot be opened lazily.** `createSyncAccessHandle()` returns a
// promise, and nothing can be awaited from inside the blocking assembly, so every
// handle a run will need is opened before it starts and closed after. They are
// exclusive locks: a leaked handle makes the *next* run fail to open the same
// cell, which is why `close()` runs in a `finally` and is idempotent.
//
// ## What a cached cell is trusted for
//
// A file's name **is** its SHA-256, and it was written only after `fetchVerified`
// checked the catalog's length and digest over exactly those bytes. So a present
// file of the right size is a cell the catalog vouched for, and re-hashing it on
// every run would cost seconds of CPU and a full read of everything this change
// exists to avoid reading — to defend against an attacker who can write another
// origin's OPFS, which is to say one who can already replace the page's own code.
// The size check stays because it is free and catches the one realistic failure:
// a write torn by a crash or a quota refusal, which leaves a short file.
//
// ## Lifecycle
//
// Cells live under `obc-cells/<revision>/`, where the revision is derived from the
// catalog's own pins on its cell indices ({@link cellStoreRevision}). A re-bake
// changes those pins, so its cells land in a fresh directory and the previous
// one's are swept the next time a store is opened. The builder may also discard
// the current revision after the run; keeping it for future builds is an explicit
// user choice.
//
// ## The other direction: the assembled map (#1116 D1)
//
// The same file system, the same worker, the same sync handles — pointed the other
// way. {@link openMapSink} is where the assembled `.obcm` goes instead of into wasm
// memory, and it is what makes a country assemblable in a tab at all.
//
// That was true when a map was a set of shards, because the **core shard could not
// be split** — one nav graph, one file. A map is now *one* file outright, so the
// same argument binds harder rather than softer: a DACH map is a single ~9 GiB
// object, which is not merely awkward in a 4 GiB wasm32 address space but larger
// than the whole of it. Writing it straight to a `FileSystemSyncAccessHandle` is
// the only shape in which that selection exists.
//
// One thing about the platform shapes this: `createSyncAccessHandle()` is
// asynchronous, and the engine writes *during* the blocking assembly, where nothing
// can be awaited. So the handle cannot be opened on demand — it is opened before the
// run, under a fixed scratch name ({@link MAP_ENTRY}). The map's real filename is
// never on this disk: it is the name the page saves the file *as*, and the page owns
// it (the assembler names nothing).

import type { Catalog } from "../catalog/manifest";

/** Where every generation of cached cells lives, under the origin's private root. */
const ROOT = "obc-cells";
/** …and where an assembly's map is written (#1116 D1). A sibling of {@link ROOT}
 *  rather than a child: cells are keyed by a catalog revision and swept when it
 *  moves, output belongs to one run and is swept when the next starts. */
const OUT = "obc-out";
/** The write-side probe's file. A dot-name so it cannot collide with a digest. */
const PROBE = ".probe";
const PROBE_BYTES = new Uint8Array([0x4f, 0x42, 0x43, 0x32]); // "OBC2"

/**
 * The one entry an assembly's output lives in, under {@link OUT}.
 *
 * A fixed scratch name, not the map's filename: the handle is opened before the run
 * and the engine never names anything anyway. Fixed rather than posted from the
 * worker because both sides can then simply agree — nothing has to carry an entry
 * name across the port, and the page has no untrusted string to resolve against a
 * directory.
 */
const MAP_ENTRY = "map.part";

// --- the platform, as far as this module uses it ------------------------------
//
// TypeScript's DOM library does not declare `createSyncAccessHandle` (it is
// worker-only) or the directory iterator, and the app's `lib` is not the place to
// fix that. These are the exact shapes used below, so a browser that has them
// answers and one that does not fails the probe.

interface SyncHandle {
    read(into: ArrayBufferView, options: { at: number }): number;
    write(from: ArrayBufferView, options: { at: number }): number;
    truncate(size: number): void;
    flush(): void;
    close(): void;
}

interface Writable {
    write(data: BufferSource): Promise<void>;
    close(): Promise<void>;
}

interface FileEntry {
    createWritable(options?: { keepExistingData?: boolean }): Promise<Writable>;
    createSyncAccessHandle(): Promise<SyncHandle>;
    getFile(): Promise<OpfsFile>;
}

/** What `getFile()` answers with. A `File` in the browser — which is a `Blob`, so it
 *  can be handed to a download without its bytes ever entering the heap. Typed as
 *  the two members this module uses plus the optional `Blob` shape, because the
 *  test's model has no `File`. */
interface OpfsFile {
    size: number;
    arrayBuffer(): Promise<ArrayBuffer>;
}

interface Directory {
    getFileHandle(name: string, options?: { create?: boolean }): Promise<FileEntry>;
    getDirectoryHandle(name: string, options?: { create?: boolean }): Promise<Directory>;
    removeEntry(name: string, options?: { recursive?: boolean }): Promise<void>;
    entries(): AsyncIterableIterator<[string, { kind: string }]>;
}

/** The origin's private root, or `null` where there is none (no secure context,
 *  an old browser, a locked-down profile). Never throws — the caller's answer to
 *  "no store" is the same as its answer to "a broken store". */
async function opfsRoot(): Promise<Directory | null> {
    // Through `unknown` because the DOM library's own `FileSystemFileHandle` is a
    // strict subset of the shapes above — it is missing the very method this
    // module exists to call.
    const storage = globalThis.navigator?.storage as unknown as
        | { getDirectory?: () => Promise<Directory> }
        | undefined;
    if (!storage?.getDirectory) return null;
    try {
        return await storage.getDirectory();
    } catch {
        return null;
    }
}

// --- which generation of cells this is ----------------------------------------

/**
 * A short, stable name for the cells a catalog publishes.
 *
 * Derived from the root's own pins — one digest per band's cell index, plus the
 * terrain index — because those change exactly when a cell's *content* might
 * have, and not when a skin is retouched or a region renamed. `generated_at`
 * would be simpler and would throw the cache away on every bake.
 *
 * FNV-1a rather than SHA-256 so it stays synchronous: this names a directory, it
 * does not authenticate anything (the filenames inside it do that).
 */
export function cellStoreRevision(catalog: Catalog): string {
    const pins = [
        ...catalog.cell_index.map((c) => `${c.band}:${c.sha256}`),
        catalog.terrain ? `terrain:${catalog.terrain.cell_index.sha256}` : "",
    ].join("|");
    let hi = 0x811c9dc5;
    let lo = 0x811c9dc5;
    for (let i = 0; i < pins.length; i++) {
        hi = Math.imul(hi ^ pins.charCodeAt(i), 0x01000193) >>> 0;
        lo = Math.imul(lo ^ pins.charCodeAt(pins.length - 1 - i), 0x01000193) >>> 0;
    }
    return `r${hi.toString(16).padStart(8, "0")}${lo.toString(16).padStart(8, "0")}`;
}

// --- the write side (main thread) ---------------------------------------------

/** Where a run's downloaded cells are kept, and what is already there. */
export interface CellStore {
    /** The revision directory these cells live in — the worker opens the same one. */
    readonly revision: string;
    /** Whether `key` is present at exactly `bytes` bytes. A short file is a torn
     *  write and answers `false`, so the cell is fetched again over it. */
    has(key: string, bytes: number): Promise<boolean>;
    /** Write one verified cell. Overwrites, so a re-fetch heals a torn file. */
    put(key: string, bytes: Uint8Array): Promise<void>;
}

/**
 * Open (creating if needed) the store for one catalog revision, and sweep the
 * others away.
 *
 * The sweep is here rather than on a timer or a setting because this is the one
 * moment the current revision is known and nothing is reading the old ones. It is
 * best-effort: a browser that will not enumerate the root simply keeps them, which
 * costs disk and nothing else.
 *
 * Returns `null` where OPFS is unusable — the caller's cue to keep the cells in
 * memory, exactly as it did before this existed.
 */
export async function openCellStore(revision: string): Promise<CellStore | null> {
    const root = await opfsRoot();
    if (!root) return null;
    let dir: Directory;
    try {
        const home = await root.getDirectoryHandle(ROOT, { create: true });
        await sweep(home, revision);
        dir = await home.getDirectoryHandle(revision, { create: true });
    } catch {
        return null;
    }
    return {
        revision,
        async has(key, bytes) {
            try {
                const file = await (await dir.getFileHandle(key)).getFile();
                return file.size === bytes;
            } catch {
                return false;
            }
        },
        async put(key, bytes) {
            const handle = await dir.getFileHandle(key, { create: true });
            const writable = await handle.createWritable();
            // No `try`/`abort`: a failed write leaves a file of the wrong size,
            // which `has` already treats as absent. Swallowing the error here
            // would instead leave the run believing a cell is cached.
            await writable.write(bytes as unknown as BufferSource);
            await writable.close();
        },
    };
}

/** Delete every downloaded map cell for this origin. Working output and scratch are separate. */
export async function clearCellStores(): Promise<void> {
    const root = await opfsRoot();
    if (!root) return;
    await removeDirectoryIfPresent(root, ROOT);
}

/** Delete downloaded cells plus assembly output and scratch left by an interrupted/previous run. */
export async function clearMapWorkStorage(): Promise<void> {
    const root = await opfsRoot();
    if (!root) return;
    for (const name of [ROOT, OUT, SCRATCH]) await removeDirectoryIfPresent(root, name);
}

/** Delete only the assembled-map staging area, leaving reusable cells and merge scratch alone. */
export async function discardMapOutput(): Promise<void> {
    const root = await opfsRoot();
    if (!root) return;
    await removeDirectoryIfPresent(root, OUT);
}

/** Delete one run's cells after its readers have closed. Used when future reuse was not selected. */
export async function discardCellStore(revision: string): Promise<void> {
    const root = await opfsRoot();
    if (!root) return;
    let home: Directory;
    try {
        home = await root.getDirectoryHandle(ROOT);
    } catch {
        return;
    }
    await removeDirectoryIfPresent(home, revision);
}

/** Ignore a genuinely absent directory, but surface locks and storage failures to the caller. */
async function removeDirectoryIfPresent(parent: Directory, name: string): Promise<void> {
    try {
        await parent.removeEntry(name, { recursive: true });
    } catch (cause) {
        try {
            await parent.getDirectoryHandle(name);
        } catch {
            return;
        }
        throw cause;
    }
}

/** Delete every revision directory except `keep`, and the probe files beside them. */
async function sweep(home: Directory, keep: string): Promise<void> {
    const stale: string[] = [];
    for await (const [name, entry] of home.entries()) {
        if (entry.kind === "directory" && name !== keep) stale.push(name);
    }
    for (const name of stale) {
        // One failure must not strand the rest — a directory can be locked by
        // another tab's still-open sync handles.
        await home.removeEntry(name, { recursive: true }).catch(() => {});
    }
}

/**
 * Whether this browser will let the main thread write cells at all — probed by
 * writing and reading back, not by sniffing for a method name.
 *
 * Memoized: it creates a file, and the answer cannot change within a page.
 */
export function cellStoreWritable(): Promise<boolean> {
    writeProbe ??= probeWrite();
    return writeProbe;
}

let writeProbe: Promise<boolean> | null = null;

async function probeWrite(): Promise<boolean> {
    const root = await opfsRoot();
    if (!root) return false;
    try {
        const home = await root.getDirectoryHandle(ROOT, { create: true });
        const handle = await home.getFileHandle(PROBE, { create: true });
        const writable = await handle.createWritable();
        await writable.write(PROBE_BYTES as unknown as BufferSource);
        await writable.close();
        return (await handle.getFile()).size === PROBE_BYTES.length;
    } catch {
        return false;
    }
}

/**
 * Whether the origin has room for `bytes` more, with a margin.
 *
 * A quota refusal mid-download is a poor failure — half a country fetched and a
 * run that has to start over in memory — so the question is asked once, before
 * anything is fetched. A browser that will not estimate gets the benefit of the
 * doubt: the write path still reports its own failures.
 */
export async function hasRoomFor(bytes: number): Promise<boolean> {
    const storage = globalThis.navigator?.storage;
    if (!storage?.estimate) return true;
    try {
        const { quota, usage } = await storage.estimate();
        if (quota === undefined) return true;
        return quota - (usage ?? 0) > bytes * 1.1;
    } catch {
        return true;
    }
}

// --- the read side (dedicated worker only) ------------------------------------

/**
 * The bytes of one run's cells, addressable synchronously.
 *
 * `read` is what the wasm engine calls from inside its blocking `run()`, by slot —
 * the cell's index in the `keys` this was opened with.
 */
export interface CellReader {
    /** Fill `into` from `offset` of the cell in `slot`. `false` means the read
     *  failed, which fails the assembly as `io` naming the cell. */
    read(slot: number, offset: number, into: Uint8Array): boolean;
    /** Release every handle. Idempotent, and **required**: a handle is an
     *  exclusive lock on its file, so one left open makes the next run fail to
     *  open the same cell. */
    close(): void;
    /** How many handles are open. Diagnostics, and what the release test asserts. */
    readonly open: number;
}

/** How many handles are opened at once. Each is a promise and a file descriptor;
 *  a country's selection is ~1000 cells, and opening them one at a time would add
 *  a second to every run for no reason. */
const OPEN_CONCURRENCY = 16;

/**
 * Open a sync access handle for every key, in order.
 *
 * **All of them, before the run** — the alternative does not exist, because
 * `createSyncAccessHandle()` is asynchronous and the assembly it would be opened
 * from cannot await. A country-scale selection therefore holds ~1000 open handles
 * for the length of a run; they are closed together by {@link CellReader.close}.
 *
 * A key that is missing or locked rejects here, before the assembly starts, with
 * the key in the message.
 */
export async function openCellReader(revision: string, keys: readonly string[]): Promise<CellReader> {
    const root = await opfsRoot();
    if (!root) throw new Error("this browser has no origin private file system to read the downloaded cells from");
    const home = await root.getDirectoryHandle(ROOT);
    const dir = await home.getDirectoryHandle(revision);

    // One handle per *file*, then one slot per cell pointing at it. A key can in
    // principle repeat — the name is the content digest, so two selected cells
    // with byte-identical content share it — and opening the same file twice is
    // a lock error, not a second handle.
    const distinct = [...new Set(keys)];
    const opened = new Map<string, SyncHandle>();
    let cursor = 0;
    const worker = async (): Promise<void> => {
        for (;;) {
            const at = cursor++;
            if (at >= distinct.length) return;
            const key = distinct[at];
            try {
                opened.set(key, await (await dir.getFileHandle(key)).createSyncAccessHandle());
            } catch (cause) {
                throw new Error(
                    `the downloaded cell ${key} could not be opened (${cause instanceof Error ? cause.message : String(cause)})`,
                );
            }
        }
    };
    // Resolved once, after every handle is open, so the read path is an array
    // index rather than a hash lookup.
    const bySlot: (SyncHandle | undefined)[] = [];
    const reader: CellReader = {
        read: counted(
            () => ioStats.cellRead,
            (_slot: number, _offset: number, into: Uint8Array) => into.byteLength,
            (slot: number, offset: number, into: Uint8Array) => {
                const handle = bySlot[slot];
                if (!handle) return false;
                try {
                    // A short read is a failure, not a partial success: the engine
                    // asked for a byte range and half of one is not it.
                    return handle.read(into, { at: offset }) === into.byteLength;
                } catch {
                    return false;
                }
            },
        ),
        close() {
            bySlot.length = 0;
            for (const handle of opened.values()) {
                try {
                    handle.close();
                } catch {
                    // Already closed, or the storage went away. Either way the
                    // remaining handles still have to be released.
                }
            }
            opened.clear();
        },
        get open() {
            return opened.size;
        },
    };
    try {
        await Promise.all(Array.from({ length: Math.min(OPEN_CONCURRENCY, distinct.length) }, worker));
    } catch (cause) {
        // Whatever did open is a lock nobody will ever use. Releasing it here is
        // what keeps a failed run from poisoning the retry.
        reader.close();
        throw cause;
    }
    for (const key of keys) bySlot.push(opened.get(key));
    return reader;
}

/**
 * Read whole cells back into memory, in the order of `keys`.
 *
 * The fallback for a browser that has OPFS but no synchronous reads: the download
 * still resumed from disk and still verified once, and the assembly's residency is
 * what it was before any of this — every cell in the heap at once. Deliberately
 * not clever; the clever path is {@link openCellReader}.
 */
export async function readCellBytes(revision: string, keys: readonly string[]): Promise<Uint8Array[]> {
    const root = await opfsRoot();
    if (!root) throw new Error("this browser has no origin private file system to read the downloaded cells from");
    const dir = await (await root.getDirectoryHandle(ROOT)).getDirectoryHandle(revision);
    const out: Uint8Array[] = [];
    for (const key of keys) {
        try {
            out.push(new Uint8Array(await (await (await dir.getFileHandle(key)).getFile()).arrayBuffer()));
        } catch (cause) {
            throw new Error(
                `the downloaded cell ${key} could not be read back (${cause instanceof Error ? cause.message : String(cause)})`,
            );
        }
    }
    return out;
}

// --- I/O accounting (dedicated worker only) -----------------------------------

/** One channel's tally: how often the engine crossed into OPFS, and what it cost. */
export interface IoCounter {
    calls: number;
    bytes: number;
    ms: number;
}

/**
 * The run's I/O ledger, by channel. The wasm engine's every OPFS crossing lands
 * in one of these — which is exactly the number an in-tab assembly's wall clock
 * is made of, and the thing a slowness report needs before anyone theorizes.
 * Read-and-reset by {@link takeIoStats}; the worker sends it with `done`. The
 * `performance.now()` pair per call is nanoseconds against calls that cost
 * microseconds.
 */
export interface IoStats {
    cellRead: IoCounter;
    sinkWrite: IoCounter;
    sinkRead: IoCounter;
    scratchWrite: IoCounter;
    scratchRead: IoCounter;
}

const counter = (): IoCounter => ({ calls: 0, bytes: 0, ms: 0 });

function freshIoStats(): IoStats {
    return {
        cellRead: counter(),
        sinkWrite: counter(),
        sinkRead: counter(),
        scratchWrite: counter(),
        scratchRead: counter(),
    };
}

let ioStats: IoStats = freshIoStats();

/** The ledger so far, handing the caller the totals and starting a fresh one. */
export function takeIoStats(): IoStats {
    const taken = ioStats;
    ioStats = freshIoStats();
    return taken;
}

function counted<A extends unknown[]>(
    tally: () => IoCounter,
    size: (...args: A) => number,
    op: (...args: A) => boolean,
): (...args: A) => boolean {
    return (...args) => {
        const t0 = performance.now();
        const ok = op(...args);
        const c = tally();
        c.calls += 1;
        c.ms += performance.now() - t0;
        if (ok) c.bytes += size(...args);
        return ok;
    };
}

// --- the write side of the output (dedicated worker only) ---------------------

/**
 * Where one assembly's map goes instead of into wasm memory (#1116 D1).
 *
 * The four byte-moving methods are the wasm sink seam verbatim, called from **inside**
 * the blocking assembly: `create`/`write`/`seal` on the way out, `readAt` for the
 * §4.8 read-back, all synchronous, all answering `false` rather than throwing —
 * which fails the run as `io`.
 *
 * The bytes land in the fixed scratch entry {@link MAP_ENTRY}, never under the map's
 * own filename: the handle has to be opened before the run, and the assembler names
 * nothing in any case. The page reads the file back with {@link readMapOutput} and
 * saves it under a name it chose itself.
 */
export interface MapSink {
    /** Begin the map, truncating whatever the entry held. */
    create(): boolean;
    /** Append to the map. A short write is a failure. */
    write(bytes: Uint8Array): boolean;
    /** Fill `into` from `offset` of the sealed map — the §4.8 read-back. Served
     *  through the wasm side's block cache, so this runs about once per 64 KiB
     *  rather than once per engine read. */
    readAt(offset: number, into: Uint8Array): boolean;
    /** No more bytes: flush, because §4.8 reads it back next. */
    seal(): boolean;
    /** Release the handle. Idempotent, and **required**: a handle is an exclusive
     *  lock, and the page cannot read the file back while the worker holds one. */
    close(): void;
    /** Whether the handle is still open. Diagnostics, and what the release test
     *  asserts. */
    readonly open: boolean;
}

/**
 * Open the map sink for one run: sweep whatever a previous one left, then open the
 * sync access handle the assembly will write through.
 *
 * Returns `null` where this browser cannot serve it — no OPFS, no sync handles, no
 * quota — which is the caller's cue to let the map be buffered in wasm memory
 * instead. That fallback is honest but small: it is the path a country-scale
 * selection cannot take, since the file is bigger than the address space.
 *
 * The **sweep is the point of doing it here**: a cancelled or crashed run leaves a
 * partial map on disk, and a partial map is nothing to anyone but the quota. This is
 * the one moment nothing is reading it.
 */
export async function openMapSink(): Promise<MapSink | null> {
    const root = await opfsRoot();
    if (!root) return null;
    let handle: SyncHandle | null = null;
    let written = 0;
    const sink: MapSink = {
        create() {
            if (!handle) return false;
            try {
                handle.truncate(0);
            } catch {
                return false;
            }
            written = 0;
            return true;
        },
        write: counted(
            () => ioStats.sinkWrite,
            (bytes: Uint8Array) => bytes.byteLength,
            (bytes: Uint8Array) => {
                if (!handle) return false;
                try {
                    const n = handle.write(bytes, { at: written });
                    written += n;
                    return n === bytes.byteLength;
                } catch {
                    return false;
                }
            },
        ),
        readAt: counted(
            () => ioStats.sinkRead,
            (_offset: number, into: Uint8Array) => into.byteLength,
            (offset: number, into: Uint8Array) => {
                if (!handle) return false;
                try {
                    // A short read is a failure, not a partial success — the same rule
                    // the input side has, and here it is §4.8 that would be misled.
                    return handle.read(into, { at: offset }) === into.byteLength;
                } catch {
                    return false;
                }
            },
        ),
        seal() {
            if (!handle) return false;
            try {
                // Truncated to exactly what was written, so an entry left longer by an
                // earlier run cannot leave trailing bytes past the map.
                handle.truncate(written);
                handle.flush();
                return true;
            } catch {
                return false;
            }
        },
        close() {
            try {
                handle?.close();
            } catch {
                // Already closed, or the storage went away. Either way there is no
                // lock left worth holding a reference for.
            }
            handle = null;
        },
        get open() {
            return handle !== null;
        },
    };
    try {
        const dir = await root.getDirectoryHandle(OUT, { create: true });
        await sweepOutputs(dir);
        handle = await (await dir.getFileHandle(MAP_ENTRY, { create: true })).createSyncAccessHandle();
    } catch {
        // Whatever opened is a lock nobody will use — the caller must be told "no",
        // not handed a sink that fails at the first write.
        sink.close();
        return null;
    }
    return sink;
}

/** Delete everything a previous run left in the output directory. */
async function sweepOutputs(dir: Directory): Promise<void> {
    const stale: string[] = [];
    for await (const [name] of dir.entries()) stale.push(name);
    for (const name of stale) {
        // One locked file must not strand the rest — another tab may still hold a
        // handle on it, and that is not this run's problem to solve.
        await dir.removeEntry(name, { recursive: true }).catch(() => {});
    }
}

// --- the scratch store: the engine's spill (dedicated worker only) ------------

/** Where the merge's spill lives (#1116 D2's third seam). A sibling of {@link ROOT}
 *  and {@link OUT}: cells outlive runs, output outlives the worker, scratch
 *  outlives **nothing** — it is swept at open and discarded at close. */
const SCRATCH = "obc-scratch";

/**
 * How many spill files one run can hold open at once. Like the map sink's handle
 * ({@link openMapSink}), every one is opened **before** the run — the opener is
 * async and the assembly cannot await — so this is a hard concurrent-file ceiling,
 * not a soft one.
 *
 * The number to size against is the external sort's run fan-out: a sort over `S`
 * spilled bytes at budget `B` holds `⌈S / (B/2)⌉` run files open during its merge,
 * plus the streams feeding and draining it. At the engine's 64 MiB default budget
 * a DACH-scale edge stream (~2 GiB) is ~64 runs; 128 leaves the same again for
 * the concurrent node/id streams and the next pass's output. Exhaustion is an
 * `io` refusal naming the working area — the remedy is a bigger
 * `mergeBudgetBytes`, which produces fewer, longer runs.
 */
const SCRATCH_SLOTS = 128;

/**
 * The engine's spill files, as the wasm scratch seam calls them: anonymous,
 * append-only, read back at `u64` offsets, synchronous. Ids are minted here and
 * **never reused** — a use-after-remove answers `false`/`-1` rather than serving
 * some later stream's bytes, which is the failure mode that would corrupt a merge
 * silently instead of failing it loudly.
 */
export interface ScratchFiles {
    /** Mint a spill file: a non-negative id, or `-1` when the pool is exhausted. */
    create(): number;
    /** Append to `id`. A short write is a failure. */
    append(id: number, bytes: Uint8Array): boolean;
    /** Fill `into` with exactly `into.byteLength` bytes at `offset`. A short read
     *  is a failure — a truncated spill read as data is a silently wrong map. */
    readAt(id: number, offset: number, into: Uint8Array): boolean;
    /** Bytes appended to `id` so far, or `-1` for an unknown/removed id. */
    len(id: number): number;
    /** Drop `id`; its pool slot becomes reusable, the id does not. */
    remove(id: number): boolean;
    /** Close every handle and delete the spill files. Idempotent. Call when the
     *  run ends, success or not — spill held between runs is quota held for
     *  nothing. */
    discard(): Promise<void>;
    /** How many pool handles are open. Diagnostics, and what the release test
     *  asserts. */
    readonly open: number;
}

/** The scratch name of one pool slot. Fixed, so the sweep and the pool agree. */
function scratchEntry(slot: number): string {
    return `x${slot.toString().padStart(3, "0")}.spill`;
}

/**
 * Open the spill pool for one run: sweep whatever a previous run left, then open
 * a sync access handle per slot. `null` where this browser cannot serve it —
 * the caller's cue to let the engine spill into wasm memory instead, exactly as
 * it does natively without a temp dir.
 */
export async function openScratchStore(slots = SCRATCH_SLOTS): Promise<ScratchFiles | null> {
    const root = await opfsRoot();
    if (!root) return null;
    const handles: (SyncHandle | null)[] = [];
    /** Pool slots with no live id, in LIFO order. */
    const free: number[] = [];
    /** Live id → its pool slot and append cursor. A removed id leaves the map. */
    const live = new Map<number, { slot: number; written: number }>();
    let next = 0;
    let dir: Directory | null = null;
    const sink: ScratchFiles = {
        create() {
            const slot = free.pop();
            if (slot === undefined) return -1;
            const handle = handles[slot];
            if (!handle) {
                free.push(slot);
                return -1;
            }
            try {
                handle.truncate(0);
            } catch {
                free.push(slot);
                return -1;
            }
            const id = next++;
            live.set(id, { slot, written: 0 });
            return id;
        },
        append: counted(
            () => ioStats.scratchWrite,
            (_id: number, bytes: Uint8Array) => bytes.byteLength,
            (id: number, bytes: Uint8Array) => {
                const at = live.get(id);
                const handle = at ? handles[at.slot] : null;
                if (!at || !handle) return false;
                try {
                    const n = handle.write(bytes, { at: at.written });
                    at.written += n;
                    return n === bytes.byteLength;
                } catch {
                    return false;
                }
            },
        ),
        readAt: counted(
            () => ioStats.scratchRead,
            (_id: number, _offset: number, into: Uint8Array) => into.byteLength,
            (id: number, offset: number, into: Uint8Array) => {
                const at = live.get(id);
                const handle = at ? handles[at.slot] : null;
                if (!at || !handle) return false;
                // Past-the-end reads must refuse here: the model below the engine
                // zero-fills, and zeroes that parse are the worst kind of wrong.
                if (offset + into.byteLength > at.written) return false;
                try {
                    return handle.read(into, { at: offset }) === into.byteLength;
                } catch {
                    return false;
                }
            },
        ),
        len(id) {
            return live.get(id)?.written ?? -1;
        },
        remove(id) {
            const at = live.get(id);
            if (!at) return false;
            live.delete(id);
            // The bytes are reclaimed at the slot's next `create` (truncate) or at
            // `discard`; freeing the slot is what matters mid-run.
            free.push(at.slot);
            return true;
        },
        async discard() {
            for (const handle of handles) {
                try {
                    handle?.close();
                } catch {
                    // Already closed, or the storage went away — the rest still
                    // have to be released, and the sweep below still runs.
                }
            }
            handles.length = 0;
            live.clear();
            free.length = 0;
            if (dir) await sweepOutputs(dir).catch(() => {});
        },
        get open() {
            return handles.filter((h) => h !== null).length;
        },
    };
    try {
        dir = await root.getDirectoryHandle(SCRATCH, { create: true });
        await sweepOutputs(dir);
        for (let slot = 0; slot < slots; slot++) {
            handles.push(await (await dir.getFileHandle(scratchEntry(slot), { create: true })).createSyncAccessHandle());
            free.push(slot);
        }
    } catch {
        // A half-open pool is not a store — release the locks and refuse, so the
        // caller falls back instead of failing at spill file 90.
        await sink.discard().catch(() => {});
        return null;
    }
    return sink;
}

/**
 * The written map, as a `Blob` — for the **main thread**, after the worker has closed
 * its handle.
 *
 * Nothing is read here: OPFS's `getFile()` answers with a `File`, which is a `Blob`,
 * so the page can hand a multi-gigabyte map to a download (or stream it to a picked
 * folder) without its bytes ever entering the tab's heap. That is the second half of
 * what D1 buys — the first is that they never entered wasm's.
 */
export async function readMapOutput(): Promise<Blob> {
    const root = await opfsRoot();
    if (!root) throw new Error("this browser has no origin private file system to read the assembled map back from");
    const dir = await root.getDirectoryHandle(OUT);
    const file = await (await dir.getFileHandle(MAP_ENTRY)).getFile();
    // The local `FileEntry` names only the two members this module calls; the real
    // `getFile()` returns a `File`, and a `File` is a `Blob`.
    return file as unknown as Blob;
}

/**
 * Whether *this* thread can read cells synchronously — the capability the whole
 * streamed path rests on, probed by writing and reading a scratch file through a
 * sync handle rather than by looking for the method.
 *
 * Only ever true in a dedicated worker. Memoized for the same reason as
 * {@link cellStoreWritable}.
 */
export function syncReadsAvailable(): Promise<boolean> {
    readProbe ??= probeSyncReads();
    return readProbe;
}

let readProbe: Promise<boolean> | null = null;

async function probeSyncReads(): Promise<boolean> {
    const root = await opfsRoot();
    if (!root) return false;
    let handle: SyncHandle | null = null;
    try {
        const home = await root.getDirectoryHandle(ROOT, { create: true });
        const file = await home.getFileHandle(`${PROBE}-sync`, { create: true });
        handle = await file.createSyncAccessHandle();
        handle.truncate(0);
        handle.write(PROBE_BYTES, { at: 0 });
        handle.flush();
        const back = new Uint8Array(PROBE_BYTES.length);
        return handle.read(back, { at: 0 }) === back.length && back.every((b, i) => b === PROBE_BYTES[i]);
    } catch {
        return false;
    } finally {
        try {
            handle?.close();
        } catch {
            // Nothing left to release.
        }
    }
}
