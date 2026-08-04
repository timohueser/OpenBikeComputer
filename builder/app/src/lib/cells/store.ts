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
// one's are swept the next time a store is opened. Nothing accumulates, and there
// is no setting to get wrong.

import type { Catalog } from "../catalog/manifest";

/** Where every generation of cached cells lives, under the origin's private root. */
const ROOT = "obc-cells";
/** The write-side probe's file. A dot-name so it cannot collide with a digest. */
const PROBE = ".probe";
const PROBE_BYTES = new Uint8Array([0x4f, 0x42, 0x43, 0x32]); // "OBC2"

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
    getFile(): Promise<{ size: number; arrayBuffer(): Promise<ArrayBuffer> }>;
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
        read(slot, offset, into) {
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
