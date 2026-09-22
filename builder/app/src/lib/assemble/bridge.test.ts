/**
 * The drift guard between the browser assembly path and the native one: the fixture cells are
 * assembled here and compared with what `cargo run -p obcm-assemble` wrote from the same input. It
 * proves the wasm build agrees with the native build, not that either is correct. Regenerate the
 * fixture with the two commands in `apps/obc-web-assemble/tests/fixture.rs`.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it, vi } from "vitest";

import {
    ASSEMBLE_ERROR_CODES,
    AssembleError,
    assembleCells as assemblePairedCells,
    estimateMemory,
    initAssemble,
    wasmMemoryBytes,
} from "./bridge";
import type {
    AssembleCell,
    AssemblePhase,
    AssembleSources,
    AssembleTerrain,
    AssembleTerrainCell,
} from "./bridge";

function assembleCells(
    cells: Parameters<typeof assemblePairedCells>[0],
    schemaJson: string,
    skinJson: string,
    options?: Parameters<typeof assemblePairedCells>[4],
    ...rest: [
        Parameters<typeof assemblePairedCells>[5]?,
        Parameters<typeof assemblePairedCells>[6]?,
        Parameters<typeof assemblePairedCells>[7]?,
        Parameters<typeof assemblePairedCells>[8]?,
        Parameters<typeof assemblePairedCells>[9]?,
        Parameters<typeof assemblePairedCells>[10]?,
    ]
) {
    return assemblePairedCells(cells, schemaJson, skinJson, darkSkin, options, ...rest);
}

/** Walk up from this file to the repo root (the directory holding the fixture). */
function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "apps/obc-web-assemble/tests/fixture/cells.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const FIXTURE = join(repoRoot(), "apps/obc-web-assemble/tests/fixture");

/** The cutter's provenance sidecar, verbatim. It is also the schema document the engine parses. */
const sidecar = readFileSync(join(FIXTURE, "cells.json"), "utf8");
const skin = readFileSync(join(FIXTURE, "skin.json"), "utf8");
const darkSkin = readFileSync(join(FIXTURE, "dark-skin.json"), "utf8");

/** Every cell the sidecar lists, in the order it lists them. */
function cells(): AssembleCell[] {
    const doc = JSON.parse(sidecar) as { cells: { id: string; band: string; path: string; partial?: boolean }[] };
    return doc.cells.map((c) => ({
        id: c.id,
        band: c.band,
        partial: c.partial ?? false,
        bytes: new Uint8Array(readFileSync(join(FIXTURE, c.path))),
    }));
}

/** The fixture's terrain sidecar — the catalog's lattice and its published cells. */
const terrainSidecar = JSON.parse(readFileSync(join(FIXTURE, "terrain.json"), "utf8")) as {
    posting_log2: number;
    cell_log2: number;
    cells: { id: string; path: string; sha256: string }[];
};

/** The fixture's fourth square is canonically void, so it has no object at all. */
function terrain(): { lattice: AssembleTerrain; cells: AssembleTerrainCell[] } {
    return {
        lattice: { postingLog2: terrainSidecar.posting_log2, cellLog2: terrainSidecar.cell_log2 },
        cells: terrainSidecar.cells.map((c) => ({
            id: c.id,
            sha256: c.sha256,
            bytes: new Uint8Array(readFileSync(join(FIXTURE, c.path))),
        })),
    };
}

/** `map.obcm` carries the spliced raster; `flat.obcm` is the same selection without terrain. */
function expectedMap(variant: "map" | "flat"): Uint8Array {
    const path = join(FIXTURE, "expected", `${variant}.obcm`);
    expect(existsSync(path), `${path} is missing — see apps/obc-web-assemble/tests/fixture.rs`).toBe(true);
    return new Uint8Array(readFileSync(path));
}

/** Fail on the first differing byte with its index and both values, instead of dumping two
 *  multi-KB arrays. */
function expectSameBytes(actual: Uint8Array, want: Uint8Array, what: string): void {
    const n = Math.min(actual.length, want.length);
    for (let i = 0; i < n; i++) {
        if (actual[i] !== want[i]) {
            throw new Error(
                `${what}: first difference at byte ${i} — wasm produced 0x${actual[i].toString(16)}, ` +
                    `the native CLI has 0x${want[i].toString(16)} (lengths ${actual.length} vs ${want.length})`,
            );
        }
    }
    expect(actual.length, `${what}: length`).toBe(want.length);
}

const OPTIONS = { acceptPartial: true };

/**
 * The fixture's cells as the browser has them: identities here, bytes behind a synchronous read
 * callback. That callback is `FileSystemSyncAccessHandle.read()` in the browser, which Node lacks.
 */
class Reads {
    readonly blobs = cells().map((c) => c.bytes);
    /** Every call, so the block cache's effect can be counted rather than assumed. */
    readonly calls: { slot: number; offset: number; length: number }[] = [];
    /** Refuse every read of this slot, as a closed handle does. */
    refuse: number | null = null;
    /** …and throw on this one, which a broken handle does instead. */
    throwAt: number | null = null;

    sources(): AssembleSources {
        return {
            cells: cells().map((c, i) => ({
                id: c.id,
                band: c.band,
                partial: c.partial,
                byteLength: this.blobs[i].length,
                key: `cell-${i}`,
            })),
            read: (slot, offset, into) => {
                this.calls.push({ slot, offset, length: into.byteLength });
                if (slot === this.throwAt) throw new Error("the sync access handle is closed");
                if (slot === this.refuse) return false;
                into.set(this.blobs[slot].subarray(offset, offset + into.byteLength));
                return true;
            },
        };
    }
}

beforeAll(async () => {
    // The `--target web` glue fetches the module relative to itself and Node cannot fetch a `file:`
    // URL, so hand it the bytes. A missing artifact is a setup error, never a skip.
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "pkg", "obc_web_assemble_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(
            `the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\` in ` +
                "builder/app (needs a Rust toolchain + wasm-pack).",
        );
    }
    await initAssemble(readFileSync(wasm));
});

describe("assembleCells", () => {
    it("reproduces the native CLI's bytes", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], terrain());
        expect(result.resident).toBe(true);
        const want = expectedMap("map");
        expect(result.byteLength).toBe(want.length);
        expect(result.sha256).toMatch(/^[0-9a-f]{64}$/);
        expectSameBytes(result.take(), want, "the assembled map");
        expect(result.warnings).toEqual([]);
        result.release();
    });

    it("keeps known-empty edge cells in bbox and coverage arithmetic without buffers", async () => {
        const base = await assembleCells(cells(), sidecar, skin, OPTIONS);
        const baseBox = base.summary.assembly_bbox_udeg as { span_log2: number };
        const baseCells = base.summary.cells;
        base.release();

        const result = await assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [
            { id: "20/0301/0264", band: "coarse" },
            { id: "18/1204/1056", band: "fine" },
            { id: "18/1204/1056", band: "network" },
        ]);
        const expanded = result.summary.assembly_bbox_udeg as { span_log2: number };
        expect(expanded.span_log2).toBeGreaterThan(baseBox.span_log2);
        expect(result.summary.cells).toBe(baseCells + 3);
        result.release();
    });

    it("rejects a known-empty identity that duplicates an artifact", async () => {
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [
                { id: "18/1204/1052", band: "fine" },
            ]),
        ).rejects.toMatchObject({ code: "input" });
    });

    /** Cells handed over as identities and read a block at a time, the way the browser reads them
     *  out of OPFS. The CLI's bytes prove the streamed path is not a different assembler. */
    it("reproduces the native CLI's bytes with the cells read from outside wasm memory", async () => {
        const store = new Reads();
        const result = await assembleCells(
            [],
            sidecar,
            skin,
            OPTIONS,
            undefined,
            [],
            terrain(),
            store.sources(),
        );
        expectSameBytes(result.take(), expectedMap("map"), "the assembled map, streamed in");
        // A path that found the bytes some other way would pass the comparison above and prove
        // nothing.
        expect(new Set(store.calls.map((c) => c.slot)).size).toBe(cells().length);
        result.release();
    });

    it("serves the engine's reads from a block cache, without changing a byte", async () => {
        const run = async (readBlockBytes?: number) => {
            const store = new Reads();
            const result = await assembleCells(
                [],
                sidecar,
                skin,
                { ...OPTIONS, readBlockBytes },
                undefined,
                [],
                undefined,
                store.sources(),
            );
            const bytes = result.take();
            result.release();
            return { bytes, calls: store.calls.length };
        };
        const uncached = await run(1);
        const cached = await run();
        expectSameBytes(uncached.bytes, cached.bytes, "the map with the read cache off");
        expect(cached.calls * 10).toBeLessThan(uncached.calls);
        // …and the reads it does make are whole blocks, not the engine's 30-byte records.
        expect(cached.calls).toBeGreaterThan(0);
    });

    it("fails as io naming the cell when a read refuses", async () => {
        const store = new Reads();
        store.refuse = 1;
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, store.sources()),
        ).rejects.toMatchObject({ code: "io" });
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, store.sources()),
        ).rejects.toThrow(new RegExp(cells()[1].id.replace(/\//g, "/")));
    });

    /** It must not escape as an unclassified wasm exception: the run's cleanup branches on the code. */
    it("fails as io when a read throws", async () => {
        const store = new Reads();
        store.throwAt = 0;
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, store.sources()),
        ).rejects.toMatchObject({ code: "io" });
    });

    /** A `FileSystemSyncAccessHandle` is an exclusive lock, so a leaked one makes the next run fail
     *  to open the same cell. This is the wasm side's half: no state may cross between runs. */
    it("assembles the same bytes on a second sequential run through the same reader", async () => {
        const store = new Reads();
        const once = async () => {
            const result = await assembleCells(
                [],
                sidecar,
                skin,
                OPTIONS,
                undefined,
                [],
                undefined,
                store.sources(),
            );
            const bytes = result.take();
            result.release();
            return bytes;
        };
        const first = await once();
        const second = await once();
        expectSameBytes(second, first, "the map, run two");
    });

    it("reports the §4.8 verify pass it already ran", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        expect(result.summary.cells).toBe(5);
        // A result the caller can hand to a device because the read-back happened in the tab.
        expect(result.summary.verified?.chunks).toBeGreaterThan(0);
        expect(result.summary.verified?.features).toBeGreaterThan(0);
        // The summary's digest and the result's are one identity, not two readings of it.
        expect(result.summary.sha256).toMatch(/^[0-9a-f]{64}$/);
        expect(result.sha256).toBe(result.summary.sha256);
        expect(result.byteLength).toBe(result.summary.bytes);
        result.release();
    });

    it("splices the raster into the map and prices it exactly", async () => {
        const withRaster = await assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], terrain());
        const t = withRaster.summary.terrain as { bytes: number; cells: number; slots: number };
        // 32-byte header + a 2 × 2 directory + three of four squares present.
        expect(t.bytes).toBe(32 + 16 + 3 * 2048);
        expect([t.cells, t.slots]).toEqual([3, 4]);
        const bytes = withRaster.take();
        withRaster.release();

        // `expected/flat.obcm` is a control rather than a second map.
        const flat = await assembleCells(cells(), sidecar, skin, OPTIONS);
        const flatBytes = flat.take();
        flat.release();
        expect(bytes.length - flatBytes.length).toBeGreaterThanOrEqual(t.bytes);
        const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        const unit = 2 ** bytes[40];
        const terrainOffset = view.getUint32(41, true) * unit;
        const terrainLength = view.getUint32(45, true) * unit;
        expect(new TextDecoder("latin1").decode(bytes.subarray(terrainOffset, terrainOffset + 4))).toBe("OBCT");
        expect(terrainLength).toBeGreaterThanOrEqual(t.bytes);
    });

    it("writes an empty terrain region when the catalog publishes no raster", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        expect(result.summary.terrain).toBeNull();
        expectSameBytes(result.take(), expectedMap("flat"), "the flat map");
        result.release();
    });

    it("reports every phase in order, never going backwards", async () => {
        const seen: { phase: AssemblePhase; fraction: number }[] = [];
        await assembleCells(cells(), sidecar, skin, OPTIONS, (phase, fraction) => {
            seen.push({ phase, fraction });
        });
        const order = seen.filter((s, i) => i === 0 || seen[i - 1].phase !== s.phase).map((s) => s.phase);
        expect(order).toEqual(["open", "poi", "nav", "plan", "write", "verify", "done"]);
        for (let i = 1; i < seen.length; i++) {
            expect(seen[i].fraction).toBeGreaterThanOrEqual(seen[i - 1].fraction);
        }
        expect(seen.at(-1)?.fraction).toBe(1);
    });

    it("aborts when the progress callback asks it to, as a cancellation", async () => {
        const attempt = assembleCells(cells(), sidecar, skin, OPTIONS, (phase) => phase === "write");
        await expect(attempt).rejects.toBeInstanceOf(AssembleError);
        await expect(attempt).rejects.toMatchObject({ code: "aborted" });
    });

    /** The read-back is most of a country-scale run, so it is where a cancel is most likely to land,
     *  and "cancelled" must not be reported as `verify`, which callers never retry past. */
    it("honours a cancel during the verify read-back, and calls it a cancellation", async () => {
        let seenVerify = 0;
        const attempt = assembleCells(cells(), sidecar, skin, OPTIONS, (phase) => {
            // Not the boundary callback — one from inside the read loop.
            return phase === "verify" && ++seenVerify >= 3;
        });
        await expect(attempt).rejects.toMatchObject({ code: "aborted" });
        expect(seenVerify).toBe(3);
    });

    /** A bar that reaches its maximum and then waits two fifths of the run is worse than no bar. */
    it("keeps reporting through the §4.8 read-back instead of freezing", async () => {
        const verify: number[] = [];
        let beforeVerify = 0;
        await assembleCells(cells(), sidecar, skin, OPTIONS, (phase, fraction) => {
            if (phase === "verify") verify.push(fraction);
            else if (verify.length === 0) beforeVerify = fraction;
        });
        expect(beforeVerify).toBeLessThanOrEqual(0.57); // the write phase ends at 0.203 + 0.363 by weight
        expect(verify.length).toBeGreaterThanOrEqual(8);
        // The locator index deliberately makes these tiny source cells much larger.
        expect(verify.at(-1)! - verify[0]).toBeGreaterThan(0.15);
        for (let i = 1; i < verify.length; i++) expect(verify[i]).toBeGreaterThan(verify[i - 1]);
    });

    /** Losing a twenty-minute assembly to a typo in a progress bar would be worse than a dead bar,
     *  but a dead bar looks exactly like a hung assembler, so it warns once. */
    it("survives a progress callback that throws, and says so once", async () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
            let calls = 0;
            const result = await assembleCells(cells(), sidecar, skin, OPTIONS, () => {
                calls++;
                throw new Error("the progress bar is broken");
            });
            expect(calls).toBeGreaterThan(5); // it kept being called
            expect(result.byteLength).toBeGreaterThan(0); // …and the assembly still finished
            expect(warn).toHaveBeenCalledTimes(1);
            expect(warn.mock.calls[0][0]).toMatch(/progress callback threw/);
            result.release();
        } finally {
            warn.mockRestore();
        }
    });

    /** The read-back is a precondition of writing a map. The Rust side pins that the option is
     *  parsed leniently; this pins that it is ignored rather than honoured. */
    it("ignores a skipVerify that a caller smuggles in", async () => {
        const sneaky = { ...OPTIONS, skipVerify: true, skip_verify: true } as Parameters<typeof assembleCells>[3];
        const result = await assembleCells(cells(), sidecar, skin, sneaky);
        expect(result.summary.verified?.chunks).toBeGreaterThan(0);
        result.release();
    });

    /** Returning an empty array the second time writes a 0-byte file to a card and reports success. */
    it("refuses a second take() instead of handing back an empty file", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        expect(result.take().length).toBe(result.byteLength);
        expect(() => result.take()).toThrow(AssembleError);
        try {
            result.take();
        } catch (e) {
            expect((e as AssembleError).code).toBe("internal");
            expect((e as AssembleError).message).toMatch(/already taken/);
        }
        // …and the size and digest it reported before the take still read true.
        expect(result.byteLength).toBeGreaterThan(0);
        expect(result.sha256).toMatch(/^[0-9a-f]{64}$/);
        result.release();
    });

    /** Releasing twice is not an error: a `finally` block may well do both. */
    it("makes the map unavailable after a release", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        result.release();
        expect(() => result.take()).toThrow(AssembleError);
        expect(() => result.release()).not.toThrow();
        // The identity survives the release — it is a snapshot, not a live reading.
        expect(result.sha256).toMatch(/^[0-9a-f]{64}$/);
        expect(result.byteLength).toBeGreaterThan(0);
    });

    /** The failure would not be a catchable exception but the module aborting, so the second caller
     *  is refused synchronously, before the first `await`, or the guard would never see the overlap. */
    it("refuses a second assembly while one is in flight", async () => {
        const first = assembleCells(cells(), sidecar, skin, OPTIONS);
        const second = assembleCells(cells(), sidecar, skin, OPTIONS);
        await expect(second).rejects.toMatchObject({ code: "internal" });
        await expect(second).rejects.toThrow(/already running/);
        (await first).release();
        // …and the guard clears: the next one runs.
        (await assembleCells(cells(), sidecar, skin, OPTIONS)).release();
    });

    it("keeps the engine's refusal classes apart", async () => {
        // The coarse cell is `partial`, and the caller has to say so rather than discover it.
        await expect(assembleCells(cells(), sidecar, skin, {})).rejects.toMatchObject({ code: "input" });

        // A corrupt download is a *format* problem, not a selection problem.
        const corrupt = cells();
        corrupt[0].bytes[0] ^= 0xff;
        await expect(assembleCells(corrupt, sidecar, skin, OPTIONS)).rejects.toMatchObject({ code: "format" });
    });

    it("surfaces the engine's own message, not a rewritten one", async () => {
        await expect(assembleCells(cells(), sidecar, skin, {})).rejects.toThrow(/partial/);
    });
});

/**
 * The vocabularies that cross the wasm boundary as bare strings. The error codes are pinned against
 * the Rust source, because provoking all seven is not practical. A code renamed on one side only
 * would turn every `catch` that branches on it into a silent fall-through to `internal`.
 */
describe("the wire contract with driver.rs", () => {
    it("lists exactly the codes ErrorCode::as_str emits", () => {
        const driver = readFileSync(join(repoRoot(), "apps/obc-web-assemble/src/driver.rs"), "utf8");
        const arms = driver.match(/ErrorCode::\w+ => "([a-z-]+)"/g);
        expect(arms, "ErrorCode::as_str no longer looks the way this test reads it").toBeTruthy();
        const codes = arms!.map((a) => a.replace(/.*"([a-z-]+)"/, "$1"));
        expect(codes.slice().sort()).toEqual([...ASSEMBLE_ERROR_CODES].sort());
    });
});

describe("wasmMemoryBytes", () => {
    /** The seam the browser memory gate reads: what the worker reports at `done` has to be the
     *  live instance's own linear memory, not a number this module keeps beside it. */
    it("reports the instantiated module's linear memory", async () => {
        const { memory } = await (await import("./pkg/obc_web_assemble.js")).default();
        expect(wasmMemoryBytes()).toBe(memory.buffer.byteLength);
        expect(wasmMemoryBytes()).toBeGreaterThan(0);
    });
});

describe("estimateMemory", () => {
    const MB = 1_000_000;
    /** The builder's sort budget (`DownloadStep.svelte`). */
    const SORT = 256 * 1024 * 1024;
    /** An OPFS host with both escapes: cells streamed in, the map sunk straight out. */
    const ON_DISK = { inputOnDisk: true, outputSunk: true };
    /** …and one that has neither, which is what a browser without sync access handles runs. */
    const IN_MEMORY = { inputOnDisk: false, outputSunk: false };

    it("passes DACH sunk and refuses it resident", async () => {
        const dach = await estimateMemory(3000 * MB, 8500 * MB, 450 * MB, SORT, ON_DISK);
        expect(dach.fits).toBe(true);
        expect(dach.headroomBytes).toBeGreaterThan(0);

        const resident = await estimateMemory(3000 * MB, 8500 * MB, 450 * MB, SORT, IN_MEMORY);
        expect(resident.fits).toBe(false);
        expect(resident.peakBytes).toBeGreaterThan(resident.ceilingBytes);
    });

    it("prices the engine by the sort budget, not the selection", async () => {
        const small = await estimateMemory(20 * MB, 60 * MB, 0, SORT, ON_DISK);
        const dach = await estimateMemory(3000 * MB, 8500 * MB, 0, SORT, ON_DISK);
        expect(dach.engineBytes).toBe(small.engineBytes);
        const tighter = await estimateMemory(3000 * MB, 8500 * MB, 0, 64 * 1024 * 1024, ON_DISK);
        expect(tighter.engineBytes).toBeLessThan(dach.engineBytes);
    });

    it("charges the fallback for cells and spill, and refuses DACH there", async () => {
        const cells = 717 * MB;
        const buffered = await estimateMemory(271 * MB, cells, 0, SORT, { inputOnDisk: false, outputSunk: true });
        expect(buffered.inputBytes).toBe(cells);
        const dach = await estimateMemory(3000 * MB, 8500 * MB, 450 * MB, SORT, {
            inputOnDisk: false,
            outputSunk: true,
        });
        expect(dach.fits).toBe(false);
    });

    /** Held separately because a host can have one escape without the other. */
    it("charges the whole map to a run that keeps it, and nothing to one that sinks it", async () => {
        const sunk = await estimateMemory(271 * MB, 717 * MB, 0, SORT, ON_DISK);
        const kept = await estimateMemory(271 * MB, 717 * MB, 0, SORT, { inputOnDisk: true, outputSunk: false });
        expect(sunk.outputBytes).toBe(0);
        expect(kept.outputBytes).toBeGreaterThan(0);
        expect(kept.peakBytes - sunk.peakBytes).toBe(kept.outputBytes);
        // Same run otherwise: the escape moves the output term and nothing else.
        expect(kept.engineBytes).toBe(sunk.engineBytes);
        expect(kept.inputBytes).toBe(sunk.inputBytes);
    });

    /** 3 GiB of a 4 GiB address space is a desktop judgement, and a phone's per-tab allowance is
     *  nowhere near it, so the budget is the caller's to lower. */
    it("lets a caller lower the budget the verdict is measured against", async () => {
        const desktop = await estimateMemory(271 * MB, 717 * MB, 0, SORT, { inputOnDisk: true, outputSunk: false });
        const phone = await estimateMemory(271 * MB, 717 * MB, 0, SORT, { inputOnDisk: true, outputSunk: false }, 1024 ** 3);
        expect(desktop.fits).toBe(true);
        expect(phone.fits).toBe(false);
        expect(phone.peakBytes).toBe(desktop.peakBytes);
        expect(phone.budgetBytes).toBe(1024 ** 3);
        expect(phone.ceilingBytes).toBe(desktop.ceilingBytes);
    });
});

/**
 * The host's own storage, as the wasm sink seam sees it. In the browser these five methods are one
 * OPFS `FileSystemSyncAccessHandle`; Node has neither, which is why the seam takes functions.
 */
class Sink {
    bytes = new Uint8Array(0);
    isSealed = false;
    readonly sealed: { sha256: string; byteLength: number }[] = [];
    /** Flip one byte at `seal`, behind the assembler's back. */
    corrupt: number | null = null;
    /** Refuse every `readAt`, as a closed handle does. */
    refuseReads = false;
    /** How many times the host was asked for bytes, so the read-back's cache can be counted. */
    reads = 0;

    create(): boolean {
        this.bytes = new Uint8Array(0);
        this.isSealed = false;
        return true;
    }

    write(bytes: Uint8Array): boolean {
        if (this.isSealed) return false;
        const grown = new Uint8Array(this.bytes.length + bytes.byteLength);
        grown.set(this.bytes);
        grown.set(bytes, this.bytes.length);
        this.bytes = grown;
        return true;
    }

    readAt(offset: number, into: Uint8Array): boolean {
        this.reads++;
        if (this.refuseReads) return false;
        if (!this.isSealed || offset + into.byteLength > this.bytes.length) return false;
        into.set(this.bytes.subarray(offset, offset + into.byteLength));
        return true;
    }

    seal(): boolean {
        this.isSealed = true;
        if (this.corrupt !== null) this.bytes[this.corrupt] ^= 0xff;
        return true;
    }

    /** The seam as `assembleCells` takes it. */
    seam() {
        return {
            create: () => this.create(),
            write: (bytes: Uint8Array) => this.write(bytes),
            readAt: (offset: number, into: Uint8Array) => this.readAt(offset, into),
            seal: () => this.seal(),
            sealed: (map: { sha256: string; byteLength: number }) => void this.sealed.push(map),
        };
    }
}

describe("the map sink (#1116 D1)", () => {
    /** If the sunk path produces the CLI's bytes, it is not a different assembler. */
    it("writes the native CLI's map without it ever entering wasm memory", async () => {
        const sink = new Sink();
        const result = await assembleCells(
            cells(),
            sidecar,
            skin,
            OPTIONS,
            undefined,
            [],
            terrain(),
            undefined,
            sink.seam(),
        );
        // The bytes are the host's, and asking for them says so rather than handing back an empty
        // file.
        expect(result.resident).toBe(false);
        expect(() => result.take()).toThrow(AssembleError);

        expectSameBytes(sink.bytes, expectedMap("map"), "the sunk map");

        // The host wrote these bytes without ever seeing them, so what it was told is all there is.
        expect(sink.sealed).toHaveLength(1);
        expect(sink.sealed[0].sha256).toBe(result.sha256);
        expect(sink.sealed[0].byteLength).toBe(result.byteLength);
        expect(sink.sealed[0].byteLength).toBe(sink.bytes.length);
        result.release();
    });

    /** With the bytes in a `Vec`, "read the map back" and "look at the map" are the same operation.
     *  With a sink the medium is the thing that can lie, and a read-back answering out of an
     *  in-memory copy would ship a corrupt map with a clean verdict. */
    it("fails verify when the sink corrupts a byte on the way to disk", async () => {
        const sink = new Sink();
        sink.corrupt = 0; // the OBCM magic, so what refuses is unmistakably the reader
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], undefined, undefined, sink.seam()),
        ).rejects.toMatchObject({ code: "verify" });
        expect(sink.reads).toBeGreaterThan(0);
        // Verification is a precondition, so the corrupt file was never announced to the caller.
        expect(sink.sealed).toEqual([]);
    });

    it("reports a sink that cannot read the map back as io, not as a verify defect", async () => {
        const sink = new Sink();
        sink.refuseReads = true;
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], undefined, undefined, sink.seam()),
        ).rejects.toMatchObject({ code: "io" });
    });

    /** The file exists and nobody knows which bytes are in it. */
    it("fails the run when the sealed report throws", async () => {
        const sink = new Sink();
        const seam = { ...sink.seam(), sealed: () => { throw new Error("the browser's storage went away"); } };
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], undefined, undefined, seam),
        ).rejects.toMatchObject({ code: "io" });
    });

    /** Refused before a byte is written: it is a defect in the caller, not in the selection. */
    it("refuses a sink that is missing a method", async () => {
        const sink = new Sink();
        const { readAt: _dropped, ...missing } = sink.seam();
        await expect(
            assembleCells(
                cells(),
                sidecar,
                skin,
                OPTIONS,
                undefined,
                [],
                undefined,
                undefined,
                missing as unknown as Parameters<typeof assembleCells>[8],
            ),
        ).rejects.toMatchObject({ code: "internal" });
    });

    it("calls a cancel during the sunk read-back a cancellation", async () => {
        const sink = new Sink();
        let seen = 0;
        await expect(
            assembleCells(
                cells(),
                sidecar,
                skin,
                OPTIONS,
                (phase) => phase === "verify" && ++seen >= 1,
                [],
                undefined,
                undefined,
                sink.seam(),
            ),
        ).rejects.toMatchObject({ code: "aborted" });
    });
});

describe("the scratch store (#1116 D2)", () => {
    /** The browser's spill pool, modelled over buffers: minted ids, append-only streams, refusals
     *  where the real pool refuses. `failAppendAfter` is the disk filling mid-run. */
    class Scratch {
        readonly files = new Map<number, Uint8Array>();
        next = 0;
        created = 0;
        failAppendAfter = Infinity;
        appends = 0;

        seam() {
            return {
                create: () => {
                    this.created += 1;
                    const id = this.next++;
                    this.files.set(id, new Uint8Array(0));
                    return id;
                },
                append: (id: number, bytes: Uint8Array) => {
                    const have = this.files.get(id);
                    if (!have || ++this.appends > this.failAppendAfter) return false;
                    const grown = new Uint8Array(have.length + bytes.byteLength);
                    grown.set(have);
                    grown.set(bytes, have.length);
                    this.files.set(id, grown);
                    return true;
                },
                readAt: (id: number, offset: number, into: Uint8Array) => {
                    const have = this.files.get(id);
                    if (!have || offset + into.byteLength > have.length) return false;
                    into.set(have.subarray(offset, offset + into.byteLength));
                    return true;
                },
                len: (id: number) => this.files.get(id)?.length ?? -1,
                remove: (id: number) => this.files.delete(id),
            };
        }
    }

    /** The engine promises to remove what it creates, and a leak here is country-scale quota held
     *  for nothing in a real run. */
    it("spills through a wired scratch store, produces the same bytes, and removes what it made", async () => {
        const scratch = new Scratch();
        const result = await assembleCells(
            cells(),
            sidecar,
            skin,
            OPTIONS,
            undefined,
            [],
            terrain(),
            undefined,
            undefined,
            scratch.seam(),
        );
        expect(scratch.created).toBeGreaterThan(0); // the seam was genuinely exercised
        expect([...scratch.files.keys()]).toEqual([]); // …and the engine cleaned up after itself
        expectSameBytes(result.take(), expectedMap("map"), "the map, spilled through the pool");
        result.release();
    });

    it("reports a failing scratch store as io, naming the working area", async () => {
        const scratch = new Scratch();
        scratch.failAppendAfter = 0;
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], undefined, undefined, undefined, scratch.seam()),
        ).rejects.toMatchObject({ code: "io" });
    });

    it("refuses a scratch store missing a method as internal, up front", async () => {
        const half = { create: () => 0 } as unknown as Parameters<typeof assembleCells>[9];
        await expect(
            assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], undefined, undefined, undefined, half),
        ).rejects.toMatchObject({ code: "internal" });
    });
});
