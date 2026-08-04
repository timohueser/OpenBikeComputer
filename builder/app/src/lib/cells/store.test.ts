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
            async getFile() {
                return {
                    size: entry.bytes.length,
                    async arrayBuffer() {
                        return entry.bytes.slice().buffer;
                    },
                };
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
