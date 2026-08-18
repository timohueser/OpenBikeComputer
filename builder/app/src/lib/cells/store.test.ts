/**
 * The downloaded-cell store (#1116 B2), against a modelled origin private file system.
 *
 * Node has no OPFS, so the platform is stood up here — and deliberately with the two behaviours that
 * make the real one hard rather than only the ones that make it work:
 *
 *   * **A sync access handle is an exclusive lock.** Opening one twice on the same file throws, as
 *     it does in every browser. That is what turns "close the handles when the run ends" from a
 *     tidiness rule into a correctness one: the *second* run is where a leak shows up.
 *   * **A file's size is what a torn write leaves.** The store's identity check is name + size, so
 *     a short file has to read as absent.
 *
 * What this cannot prove is that a browser's OPFS behaves like the model. That is what the write and
 * sync-read probes are for — both do a real round trip through the real API before anything is
 * trusted to it.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";

import { exampleCatalog } from "../catalog/testdata";
import type { Catalog } from "../catalog/manifest";

// --- the model ----------------------------------------------------------------

class FakeFile {
    bytes = new Uint8Array(0);
    /** Open sync handles. The real thing allows exactly one. */
    locked = false;
}

class FakeSyncHandle {
    constructor(
        private readonly file: FakeFile,
        readonly onClose: () => void,
    ) {}
    closed = false;
    read(into: Uint8Array, { at }: { at: number }): number {
        if (this.closed) throw new Error("the handle is closed");
        const from = this.file.bytes.subarray(at, at + into.byteLength);
        into.set(from);
        return from.byteLength;
    }
    write(from: Uint8Array, { at }: { at: number }): number {
        if (this.closed) throw new Error("the handle is closed");
        const end = at + from.byteLength;
        if (end > this.file.bytes.length) {
            const grown = new Uint8Array(end);
            grown.set(this.file.bytes);
            this.file.bytes = grown;
        }
        this.file.bytes.set(from, at);
        return from.byteLength;
    }
    truncate(size: number) {
        this.file.bytes = this.file.bytes.slice(0, size);
    }
    flush() {}
    close() {
        this.closed = true;
        this.onClose();
    }
}

class FakeDir {
    readonly files = new Map<string, FakeFile>();
    readonly dirs = new Map<string, FakeDir>();
    /** Every sync handle ever opened here, so a test can assert they were closed. */
    static handles: FakeSyncHandle[] = [];

    async getDirectoryHandle(name: string, options?: { create?: boolean }): Promise<FakeDir> {
        const existing = this.dirs.get(name);
        if (existing) return existing;
        if (!options?.create) throw new Error(`NotFoundError: ${name}`);
        const made = new FakeDir();
        this.dirs.set(name, made);
        return made;
    }

    async getFileHandle(name: string, options?: { create?: boolean }) {
        let file = this.files.get(name);
        if (!file) {
            if (!options?.create) throw new Error(`NotFoundError: ${name}`);
            file = new FakeFile();
            this.files.set(name, file);
        }
        const entry = file;
        return {
            // A real `getFile()` answers with a `File`, which is a `Blob` — the
            // property `readMapOutput` rests on, since it is what lets the page
            // hand a gigabyte-scale shard to a download without reading it.
            async getFile() {
                return new Blob([entry.bytes.slice() as unknown as BlobPart]);
            },
            async createWritable() {
                let staged = new Uint8Array(0);
                return {
                    async write(data: Uint8Array) {
                        const grown = new Uint8Array(staged.length + data.byteLength);
                        grown.set(staged);
                        grown.set(data, staged.length);
                        staged = grown;
                    },
                    async close() {
                        entry.bytes = staged;
                    },
                };
            },
            async createSyncAccessHandle() {
                if (entry.locked) throw new Error(`NoModificationAllowedError: ${name} is already open`);
                entry.locked = true;
                const handle = new FakeSyncHandle(entry, () => (entry.locked = false));
                FakeDir.handles.push(handle);
                return handle;
            },
        };
    }

    async removeEntry(name: string, options?: { recursive?: boolean }): Promise<void> {
        void options;
        // A file another agent holds a sync handle on cannot be removed, exactly as in the browser.
        // That is what makes a sweep best-effort rather than a guarantee — and what a half-open
        // pool has to survive.
        if (this.files.get(name)?.locked) throw new Error(`NoModificationAllowedError: ${name} is open`);
        this.files.delete(name);
        this.dirs.delete(name);
    }

    async *entries(): AsyncIterableIterator<[string, { kind: string }]> {
        for (const name of this.dirs.keys()) yield [name, { kind: "directory" }];
        for (const name of this.files.keys()) yield [name, { kind: "file" }];
    }
}

/** Install a fresh OPFS and load the module against it. The store memoizes both
 *  probes, so every test gets its own module instance. */
async function withOpfs(root: FakeDir | null, quota?: { quota: number; usage: number }) {
    FakeDir.handles = [];
    vi.resetModules();
    vi.stubGlobal("navigator", {
        storage: root
            ? {
                  getDirectory: async () => root,
                  ...(quota ? { estimate: async () => quota } : {}),
              }
            : undefined,
    });
    return import("./store");
}

const opfs = () => new FakeDir();

/** What `obc-cells/<revision>/` holds, once something has been written there. */
function cellsIn(root: FakeDir, revision: string): FakeDir {
    return root.dirs.get("obc-cells")!.dirs.get(revision)!;
}

const KEY_A = "a".repeat(64);
const KEY_B = "b".repeat(64);

beforeEach(() => {
    vi.unstubAllGlobals();
});

describe("cellStoreRevision", () => {
    it("changes when the published cells could have, and not when the catalog is merely edited", async () => {
        const { cellStoreRevision } = await withOpfs(opfs());
        const base = cellStoreRevision(exampleCatalog);
        expect(base).toMatch(/^r[0-9a-f]{16}$/);
        expect(cellStoreRevision(exampleCatalog)).toBe(base);

        // A re-bake republishes the cell indices, so their pins move…
        const rebaked: Catalog = {
            ...exampleCatalog,
            cell_index: exampleCatalog.cell_index.map((c, i) => (i === 0 ? { ...c, sha256: KEY_A } : c)),
        };
        expect(cellStoreRevision(rebaked)).not.toBe(base);

        // …while a retouched skin or a renamed region does not republish a cell.
        const retouched: Catalog = { ...exampleCatalog, generated_at: "2099-01-01T00:00:00Z", regions: [] };
        expect(cellStoreRevision(retouched)).toBe(base);
    });
});

describe("the write side", () => {
    it("keeps a cell under its digest and recognises it again", async () => {
        const root = opfs();
        const { openCellStore } = await withOpfs(root);
        const store = (await openCellStore("r1"))!;
        expect(store).not.toBeNull();

        expect(await store.has(KEY_A, 4)).toBe(false);
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));
        expect(await store.has(KEY_A, 4)).toBe(true);
        expect(cellsIn(root, "r1").files.get(KEY_A)!.bytes).toEqual(new Uint8Array([1, 2, 3, 4]));
    });

    /** The identity check is name + size, and the size is the half that earns its keep: a write torn
     *  by a crash or a quota refusal leaves a short file, which must read as absent so the next run
     *  fetches over it rather than assembling from half a cell. */
    it("treats a short file as absent", async () => {
        const root = opfs();
        const { openCellStore } = await withOpfs(root);
        const store = (await openCellStore("r1"))!;
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));
        cellsIn(root, "r1").files.get(KEY_A)!.bytes = new Uint8Array([1, 2]);
        expect(await store.has(KEY_A, 4)).toBe(false);
        // …and re-fetching heals it, because `put` overwrites.
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));
        expect(await store.has(KEY_A, 4)).toBe(true);
    });

    /** A superseded bake's cells are dead weight — nothing will ask for those digests again — so
     *  opening the current revision is where they go. No timer, no setting, no growth. */
    it("sweeps the revisions it is not opening", async () => {
        const root = opfs();
        const { openCellStore } = await withOpfs(root);
        const old = (await openCellStore("r-old"))!;
        await old.put(KEY_A, new Uint8Array([1, 2, 3, 4]));

        const fresh = (await openCellStore("r-new"))!;
        await fresh.put(KEY_B, new Uint8Array([5, 6]));
        const home = root.dirs.get("obc-cells")!;
        expect([...home.dirs.keys()]).toEqual(["r-new"]);
        expect(await fresh.has(KEY_B, 2)).toBe(true);
    });

    it("discards one temporary revision without touching the store root", async () => {
        const root = opfs();
        const { discardCellStore, openCellStore } = await withOpfs(root);
        const store = (await openCellStore("r1"))!;
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));

        await discardCellStore("r1");
        expect(root.dirs.has("obc-cells")).toBe(true);
        expect(root.dirs.get("obc-cells")!.dirs.has("r1")).toBe(false);
        await expect(discardCellStore("missing")).resolves.toBeUndefined();
    });

    it("clears all downloaded cells on request", async () => {
        const root = opfs();
        const { clearCellStores, openCellStore } = await withOpfs(root);
        const store = (await openCellStore("r1"))!;
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));

        await clearCellStores();
        expect(root.dirs.has("obc-cells")).toBe(false);
        await expect(clearCellStores()).resolves.toBeUndefined();
    });

    it("clears every map-working directory on request", async () => {
        const root = opfs();
        const { clearMapWorkStorage, openCellStore } = await withOpfs(root);
        const store = (await openCellStore("r1"))!;
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4]));
        await root.getDirectoryHandle("obc-out", { create: true });
        await root.getDirectoryHandle("obc-scratch", { create: true });

        await clearMapWorkStorage();
        expect([...root.dirs.keys()]).toEqual([]);
    });

    it("reports no store where the browser has no OPFS", async () => {
        const { openCellStore, cellStoreWritable } = await withOpfs(null);
        expect(await openCellStore("r1")).toBeNull();
        expect(await cellStoreWritable()).toBe(false);
    });

    /** Probed by writing and reading back, not by looking for a method: the fallback has to be
     *  chosen on what the browser does. */
    it("probes the write side for real", async () => {
        const root = opfs();
        const { cellStoreWritable } = await withOpfs(root);
        expect(await cellStoreWritable()).toBe(true);
        expect(root.dirs.get("obc-cells")!.files.has(".probe")).toBe(true);
    });

    it("asks about quota once, with a margin, and believes a browser that will not say", async () => {
        const { hasRoomFor } = await withOpfs(opfs(), { quota: 1000, usage: 100 });
        expect(await hasRoomFor(800)).toBe(true);
        expect(await hasRoomFor(900)).toBe(false);
        const { hasRoomFor: unmeasured } = await withOpfs(opfs());
        expect(await unmeasured(1e12)).toBe(true);
    });
});

describe("the read side", () => {
    async function seeded(revision = "r1") {
        const root = opfs();
        const mod = await withOpfs(root);
        const store = (await mod.openCellStore(revision))!;
        await store.put(KEY_A, new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]));
        await store.put(KEY_B, new Uint8Array([9, 10]));
        return { root, mod };
    }

    it("reads a byte range by slot", async () => {
        const { mod } = await seeded();
        const reader = await mod.openCellReader("r1", [KEY_A, KEY_B]);
        try {
            const into = new Uint8Array(3);
            expect(reader.read(0, 2, into)).toBe(true);
            expect(into).toEqual(new Uint8Array([3, 4, 5]));
            expect(reader.read(1, 0, new Uint8Array(2))).toBe(true);
            expect(reader.open).toBe(2);
        } finally {
            reader.close();
        }
    });

    /** A short read is a failure, not a partial success: the engine asked for a byte range, and half
     *  of one silently accepted is a map assembled out of whatever was left in the buffer. */
    it("refuses a read that runs off the end", async () => {
        const { mod } = await seeded();
        const reader = await mod.openCellReader("r1", [KEY_B]);
        try {
            expect(reader.read(0, 1, new Uint8Array(4))).toBe(false);
        } finally {
            reader.close();
        }
    });

    /**
     * **The handle-release pin.** A sync access handle is an exclusive lock, so a run that leaks one
     * does not fail — the *next* run does, when it cannot open the same cell. Two sequential runs is
     * the shape that catches it, and it is the shape a rider produces by pressing the button twice.
     */
    it("releases every handle, so a second run over the same cells succeeds", async () => {
        const { mod } = await seeded();
        for (let run = 0; run < 2; run++) {
            const reader = await mod.openCellReader("r1", [KEY_A, KEY_B]);
            expect(reader.read(0, 0, new Uint8Array(4))).toBe(true);
            reader.close();
            expect(reader.open).toBe(0);
        }
        // Every handle the module opened for the cells was closed — the probe's own is closed too.
        expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([]);
    });

    /** A key is a content digest, so two cells with byte-identical content name one file — and
     *  opening it twice is a lock error, not a second handle. Vanishingly rare (a cell's header
     *  carries its own grid square), which is exactly why it would be found the hard way. */
    it("shares one handle between two slots that name the same file", async () => {
        const { mod } = await seeded();
        const reader = await mod.openCellReader("r1", [KEY_A, KEY_A]);
        try {
            expect(reader.open).toBe(1);
            const into = new Uint8Array(2);
            expect(reader.read(1, 0, into)).toBe(true);
            expect(into).toEqual(new Uint8Array([1, 2]));
        } finally {
            reader.close();
        }
    });

    it("closes twice without complaint, because the caller's finally may too", async () => {
        const { mod } = await seeded();
        const reader = await mod.openCellReader("r1", [KEY_A]);
        reader.close();
        expect(() => reader.close()).not.toThrow();
    });

    /**
     * A missing or locked cell rejects *before* the assembly starts, naming the key — and lets go of
     * whatever it had already opened, or the retry would fail on a lock this failure created.
     */
    it("names the cell it could not open, and strands no lock doing it", async () => {
        const { mod } = await seeded();
        await expect(mod.openCellReader("r1", [KEY_A, "c".repeat(64)])).rejects.toThrow(/c{64}/);
        expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([]);
        // …so the same cells open cleanly on the next attempt.
        const reader = await mod.openCellReader("r1", [KEY_A]);
        reader.close();
    });

    /** The fallback for a browser with OPFS but no sync handles: the download still resumed, and the
     *  memory profile is what it was before any of this. */
    it("reads whole cells back for a browser without sync handles", async () => {
        const { mod } = await seeded();
        expect(await mod.readCellBytes("r1", [KEY_B, KEY_A])).toEqual([
            new Uint8Array([9, 10]),
            new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
        ]);
    });

    it("probes the sync-read side for real, and releases the probe's handle", async () => {
        const { mod } = await seeded();
        expect(await mod.syncReadsAvailable()).toBe(true);
        expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([]);
        const none = await withOpfs(null);
        expect(await none.syncReadsAvailable()).toBe(false);
    });
});

describe("the map sink", () => {
    /** What `obc-out/` holds. */
    function outIn(root: FakeDir): FakeDir {
        return root.dirs.get("obc-out")!;
    }

    /** The one entry the sink writes through, which both sides know as a constant. */
    const ENTRY = "map.part";

    it("takes the map and reads it back through the same handle", async () => {
        const root = opfs();
        const { openMapSink } = await withOpfs(root);
        const sink = (await openMapSink())!;
        expect(sink).not.toBeNull();
        try {
            expect(sink.create()).toBe(true);
            expect(sink.write(new Uint8Array([1, 2, 3, 4]))).toBe(true);
            expect(sink.write(new Uint8Array([5, 6]))).toBe(true);
            expect(sink.seal()).toBe(true);
            const into = new Uint8Array(3);
            expect(sink.readAt(2, into)).toBe(true);
            expect(into).toEqual(new Uint8Array([3, 4, 5]));
            expect(outIn(root).files.get(ENTRY)!.bytes).toEqual(new Uint8Array([1, 2, 3, 4, 5, 6]));
        } finally {
            sink.close();
        }
    });

    /** A short read is a failure, not a partial success — here it is §4.8 that would be misled, and
     *  a verify pass that accepts half a read is not a verify pass. */
    it("refuses a read that runs off the end of the map", async () => {
        const { openMapSink } = await withOpfs(opfs());
        const sink = (await openMapSink())!;
        try {
            sink.create();
            sink.write(new Uint8Array([1, 2]));
            sink.seal();
            expect(sink.readAt(1, new Uint8Array(4))).toBe(false);
        } finally {
            sink.close();
        }
    });

    /** Every method answers `false` rather than throwing once the handle is gone, so a run that
     *  raced a close fails as `io` instead of trapping the worker. */
    it("refuses every operation on a closed sink", async () => {
        const { openMapSink } = await withOpfs(opfs());
        const sink = (await openMapSink())!;
        sink.close();
        expect(sink.open).toBe(false);
        expect(sink.create()).toBe(false);
        expect(sink.write(new Uint8Array(1))).toBe(false);
        expect(sink.readAt(0, new Uint8Array(1))).toBe(false);
        expect(sink.seal()).toBe(false);
    });

    /**
     * **The stale-partial sweep.** A cancelled or crashed run leaves most of a map on disk. It is
     * nothing to anyone — a partial `.obcm` fails its own header checks, and the file is only saved
     * once the run says it finished — but it is not nothing to the quota, and a country's worth of
     * it would stop the *next* run from having room to download anything. Opening the sink is the
     * one moment nothing is reading it.
     */
    it("sweeps what a cancelled run left before opening the next one", async () => {
        const root = opfs();
        const { openMapSink } = await withOpfs(root);
        const cancelled = (await openMapSink())!;
        cancelled.create();
        cancelled.write(new Uint8Array([9, 9, 9, 9]));
        // No seal: the shape a `worker.terminate()` leaves behind.
        cancelled.close();
        expect(outIn(root).files.get(ENTRY)!.bytes).toHaveLength(4);

        const fresh = (await openMapSink())!;
        try {
            expect(outIn(root).files.get(ENTRY)!.bytes).toHaveLength(0);
        } finally {
            fresh.close();
        }
    });

    /** …and a file left by something else entirely goes with it: the directory belongs to one run
     *  at a time, so anything in it when a run starts is dead. */
    it("sweeps entries the sink does not even use", async () => {
        const root = opfs();
        const { openMapSink } = await withOpfs(root);
        const home = await root.getDirectoryHandle("obc-out", { create: true });
        await home.getFileHandle("s00.part", { create: true });
        const sink = (await openMapSink())!;
        try {
            expect([...outIn(root).files.keys()]).toEqual([ENTRY]);
        } finally {
            sink.close();
        }
    });

    /** Beginning the map truncates the entry, and sealing truncates it again: a run following a
     *  longer one must not leave the previous map's tail past the end of this one. */
    it("truncates the entry when the map begins, and again when it is sealed", async () => {
        const root = opfs();
        const { openMapSink } = await withOpfs(root);
        const sink = (await openMapSink())!;
        try {
            sink.create();
            sink.write(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]));
            sink.seal();
            sink.create();
            sink.write(new Uint8Array([7, 7]));
            sink.seal();
            expect(outIn(root).files.get(ENTRY)!.bytes).toEqual(new Uint8Array([7, 7]));
        } finally {
            sink.close();
        }
    });

    /**
     * **The handle-release pin, and the page's half of it.** A sync access handle is an exclusive
     * lock: one left open makes the next run fail to open the sink *and* stops the page from ever
     * reading the map it is holding. Both endings are the same `close()`.
     */
    it("releases the handle, so the next run opens the same entry", async () => {
        const { openMapSink } = await withOpfs(opfs());
        for (let run = 0; run < 2; run++) {
            const sink = (await openMapSink())!;
            expect(sink.open).toBe(true);
            sink.close();
            expect(sink.open).toBe(false);
            expect(() => sink.close()).not.toThrow();
        }
        expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([]);
    });

    /**
     * The page's side: a `Blob` of exactly what the assembler wrote, opened once the worker has let
     * go. Nothing is read to produce it — that is the point, and it is what keeps a multi-gigabyte
     * map out of the tab's heap on its way to a download.
     */
    it("hands the page a Blob of exactly what was written", async () => {
        const root = opfs();
        const { openMapSink, readMapOutput } = await withOpfs(root);
        const sink = (await openMapSink())!;
        sink.create();
        sink.write(new Uint8Array([1, 2, 3, 4, 5]));
        sink.seal();
        sink.close();

        const blob = await readMapOutput();
        expect(blob.size).toBe(5);
        expect(new Uint8Array(await blob.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3, 4, 5]));
    });

    it("reports no sink where the browser has no OPFS, so the run keeps the map in memory", async () => {
        const { openMapSink } = await withOpfs(null);
        expect(await openMapSink()).toBeNull();
    });

    /** A sink whose handle cannot be opened is no sink: handing one back would fail the run at the
     *  first write instead of falling back to memory before it starts. */
    it("refuses a sink it could not open, and strands no lock doing it", async () => {
        const root = opfs();
        const { openMapSink } = await withOpfs(root);
        const home = await root.getDirectoryHandle("obc-out", { create: true });
        const blocked = await home.getFileHandle(ENTRY, { create: true });
        const held = await blocked.createSyncAccessHandle();
        try {
            expect(await openMapSink()).toBeNull();
            // Nothing this attempt opened was left behind — only the lock the test itself holds, or
            // the retry would fail on a lock this failure created.
            expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([held]);
        } finally {
            held.close();
        }
    });
});

describe("the scratch store", () => {
    /** What `obc-scratch/` holds. */
    function scratchIn(root: FakeDir): FakeDir {
        return root.dirs.get("obc-scratch")!;
    }

    it("round-trips a spill file: create, append, read back, len", async () => {
        const root = opfs();
        const { openScratchStore } = await withOpfs(root);
        const scratch = (await openScratchStore(2))!;
        try {
            const id = scratch.create();
            expect(id).toBeGreaterThanOrEqual(0);
            expect(scratch.append(id, new Uint8Array([1, 2, 3, 4]))).toBe(true);
            expect(scratch.append(id, new Uint8Array([5, 6]))).toBe(true);
            expect(scratch.len(id)).toBe(6);
            const into = new Uint8Array(3);
            expect(scratch.readAt(id, 2, into)).toBe(true);
            expect(into).toEqual(new Uint8Array([3, 4, 5]));
        } finally {
            await scratch.discard();
        }
    });

    /** The contract the engine's `MemoryScratch` documents and the merge depends on: a removed id
     *  refuses, it never resolves to some later stream's bytes — that failure mode is a silently
     *  wrong map, the one thing worse than a failed run. */
    it("never reuses an id — a use-after-remove refuses instead of reading another stream", async () => {
        const root = opfs();
        const { openScratchStore } = await withOpfs(root);
        const scratch = (await openScratchStore(1))!;
        try {
            const dead = scratch.create();
            expect(scratch.append(dead, new Uint8Array([9, 9]))).toBe(true);
            expect(scratch.remove(dead)).toBe(true);
            // The single pool slot is reused; the id is not, and the new stream starts empty.
            const alive = scratch.create();
            expect(alive).not.toBe(dead);
            expect(scratch.len(alive)).toBe(0);
            expect(scratch.append(alive, new Uint8Array([1]))).toBe(true);
            // Every operation on the dead id refuses.
            expect(scratch.append(dead, new Uint8Array([7]))).toBe(false);
            expect(scratch.readAt(dead, 0, new Uint8Array(1))).toBe(false);
            expect(scratch.len(dead)).toBe(-1);
            expect(scratch.remove(dead)).toBe(false);
        } finally {
            await scratch.discard();
        }
    });

    it("refuses a read past what was appended — zero-fill must not parse as data", async () => {
        const { openScratchStore } = await withOpfs(opfs());
        const scratch = (await openScratchStore(1))!;
        try {
            const id = scratch.create();
            expect(scratch.append(id, new Uint8Array([1, 2]))).toBe(true);
            expect(scratch.readAt(id, 1, new Uint8Array(2))).toBe(false);
            expect(scratch.readAt(id, 0, new Uint8Array(2))).toBe(true);
        } finally {
            await scratch.discard();
        }
    });

    it("refuses with -1 when the pool is exhausted, and recovers when a file is removed", async () => {
        const { openScratchStore } = await withOpfs(opfs());
        const scratch = (await openScratchStore(2))!;
        try {
            const a = scratch.create();
            const b = scratch.create();
            expect(a).toBeGreaterThanOrEqual(0);
            expect(b).toBeGreaterThanOrEqual(0);
            expect(scratch.create()).toBe(-1);
            expect(scratch.remove(a)).toBe(true);
            expect(scratch.create()).toBeGreaterThanOrEqual(0);
        } finally {
            await scratch.discard();
        }
    });

    it("discard releases every handle and deletes the spill — quota is not held between runs", async () => {
        const root = opfs();
        const { openScratchStore } = await withOpfs(root);
        const scratch = (await openScratchStore(2))!;
        const id = scratch.create();
        scratch.append(id, new Uint8Array(1024));
        await scratch.discard();
        expect(scratch.open).toBe(0);
        expect(FakeDir.handles.filter((h) => !h.closed)).toEqual([]);
        expect(scratchIn(root).files.size).toBe(0);
        // Idempotent, as the seam promises.
        await scratch.discard();
    });

    it("sweeps a crashed run's spill at open", async () => {
        const root = opfs();
        const { openScratchStore } = await withOpfs(root);
        const home = await root.getDirectoryHandle("obc-scratch", { create: true });
        const stale = await home.getFileHandle("x000.spill", { create: true });
        (await stale.createSyncAccessHandle()).close();
        const scratch = (await openScratchStore(1))!;
        try {
            // The stale file was removed and the slot's fresh file starts empty.
            const id = scratch.create();
            expect(scratch.len(id)).toBe(0);
        } finally {
            await scratch.discard();
        }
    });

    it("answers null with no OPFS, which is the caller's cue to spill in memory", async () => {
        const { openScratchStore } = await withOpfs(null);
        expect(await openScratchStore()).toBeNull();
    });
});
