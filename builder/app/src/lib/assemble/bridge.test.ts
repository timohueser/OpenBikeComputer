/**
 * The drift guard between the **browser** assembly path and the native one.
 *
 * These are not "does the wrapper work" tests. They exist so that a change to `obcm-assemble` — the
 * graft's relocation constants, the nav renumbering, the shard planner — cannot ship a browser build
 * that quietly disagrees with the CLI. The inputs are the checked-in cell tree in
 * `apps/obc-web-assemble/tests/fixture/` and the expected outputs are what
 * `cargo run -p obcm-assemble` wrote from them; `apps/obc-web-assemble/tests/fixture.rs` documents
 * the provenance of both, executably.
 *
 * **What that proves, precisely.** The fixture's `expected/` came from the native CLI over the same
 * cells, so these tests prove the **wasm build agrees with the native build of the same engine** — a
 * drift and miscompilation guard across the bindgen/adapter seam, and a determinism pin. They are
 * not an independent correctness check: a bug in the engine would move both together. That is the
 * right scope here — this is a second *host*, not a second implementation — and the assembler's own
 * correctness is tested where it lives, in `obcm-assemble`'s differential oracle against the real
 * packer.
 *
 * Regenerate the fixture with the two commands in `apps/obc-web-assemble/tests/fixture.rs` after a
 * *deliberate* change; `cargo test -p obc-web-assemble` holds the native side to the same bytes.
 */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { ASSEMBLE_ERROR_CODES, AssembleError, assembleCells, estimateMemory, initAssemble } from "./bridge";
import type {
    AssembleCell,
    AssemblePhase,
    AssembleSources,
    AssembleTerrain,
    AssembleTerrainCell,
} from "./bridge";

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

/**
 * The cutter's provenance sidecar, verbatim. It doubles as the **schema document**: the engine's
 * parser accepts an OBCC v2 root and `cells.json` is `{"schema": {…}, "cells": […]}` — which is
 * exactly the shape a hosted catalog hands the builder.
 */
const sidecar = readFileSync(join(FIXTURE, "cells.json"), "utf8");
const skin = readFileSync(join(FIXTURE, "skin.json"), "utf8");

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

/** The fixture's terrain sidecar — the catalog's §13.1 lattice and its published cells. */
const terrainSidecar = JSON.parse(readFileSync(join(FIXTURE, "terrain.json"), "utf8")) as {
    posting_log2: number;
    cell_log2: number;
    cells: { id: string; path: string; sha256: string }[];
};

/** The raster, as the download hands it over. The fixture's fourth square is canonically void and
 *  is deliberately absent: it has no object at all (`OBCC_Spec.md` §13.6). */
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

/**
 * What the native CLI left in `tests/fixture/<dir>/`, in the order the bridge must hand it on:
 * OBCM shards ascending, then the terrain shard, then the OBCS manifest last (OBCA §5.4). None of
 * that is the alphabet's order — `MS1.OBD` sorts before `MS1S00.OBM` — so the key is spelled out.
 */
function expected(dir: string): { name: string; bytes: Uint8Array }[] {
    const root = join(FIXTURE, dir);
    const files = readdirSync(root).map((name) => ({ name, bytes: new Uint8Array(readFileSync(join(root, name))) }));
    const rank = (name: string) => (name.endsWith(".OBS") ? 2 : name.endsWith(".OBD") ? 1 : 0);
    files.sort((a, b) => rank(a.name) - rank(b.name) || a.name.localeCompare(b.name));
    expect(files.length, `${root} is empty — see apps/obc-web-assemble/tests/fixture.rs`).toBeGreaterThan(0);
    return files;
}

/**
 * Fail on the first differing byte with its index and both values, instead of dumping two
 * multi-KB arrays. A byte-identity failure is usually one field, and the index says which.
 */
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

const OPTIONS = { name: "Bridge Fixture", acceptPartial: true };

/**
 * The fixture's cells as the browser has them after #1116 B2: identities and lengths on this side of
 * the wasm boundary, bytes behind a synchronous read callback.
 *
 * In the browser that callback is `FileSystemSyncAccessHandle.read()`, which Node does not have and
 * which is exactly why the seam takes a function: the engine's path is the same either way, and
 * backing it with buffers here makes the streamed path testable at all.
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
    // `--target web` glue resolves the module relative to itself and fetches it; Node cannot fetch a
    // `file:` URL, so hand it the bytes. A missing artifact is a setup error, never a skip — a
    // silently-skipped drift guard is worse than no drift guard.
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
    it("reproduces the native CLI's bytes for a single-file assembly", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], terrain());
        const want = expected("expected");
        expect(result.files.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of result.files.entries()) {
            expectSameBytes(file.take(), want[i].bytes, file.name);
        }
        expect(result.files.at(-1)?.role).toBe("manifest");
        expect(result.warnings).toEqual([]);
        result.release();
        // Released means released: a set can be gigabytes, so the handle must genuinely be dead.
        expect(() => result.files[0].take()).toThrow();
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

    it("reproduces the native CLI's bytes for a volume set, manifest last", async () => {
        const result = await assembleCells(cells(), sidecar, skin, { ...OPTIONS, forceSplit: true }, undefined, [], terrain());
        const want = expected("expected-split");
        expect(result.files.map((f) => f.role)).toEqual(["core", "coarse", "geometry", "terrain", "manifest"]);
        expect(result.files.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of result.files.entries()) {
            expectSameBytes(file.take(), want[i].bytes, file.name);
        }
    });

    /**
     * **The eviction pin** (#1116 B1). With a file sink, each shard crosses to JS the moment its
     * §4.8 read-back passes and its wasm-side buffer is freed there and then — so the set's
     * contribution to a tab's peak is one shard instead of all of it.
     *
     * What has to hold for that to be a saving rather than a bug: the stream, followed by whatever
     * is left at the end, is the *same set* the CLI wrote — same files, same order, same bytes — and
     * the wasm side really is holding nothing for the shards afterwards.
     */
    it("streams each shard out as it is verified and keeps nothing of it", async () => {
        const streamed: { name: string; role: string; sha256: string; bytes: Uint8Array }[] = [];
        const result = await assembleCells(
            cells(),
            sidecar,
            skin,
            { ...OPTIONS, forceSplit: true },
            undefined,
            [],
            terrain(),
            (file) => streamed.push(file),
        );
        expect(streamed.map((f) => f.role)).toEqual(["core", "coarse", "geometry"]);
        // Only the raster and the manifest were still in wasm memory at the end — the three shards
        // are gone from it, while the summary still knows all three.
        expect(result.files.map((f) => f.role)).toEqual(["terrain", "manifest"]);
        expect(result.summary.shards.length).toBe(3);

        const want = expected("expected-split");
        const delivered = [
            ...streamed.map((f) => ({ name: f.name, bytes: f.bytes })),
            ...result.files.map((f) => ({ name: f.name, bytes: f.take() })),
        ];
        expect(delivered.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of delivered.entries()) expectSameBytes(file.bytes, want[i].bytes, file.name);

        // The digest a shard was handed over with is the one the manifest records for it — the
        // identity a caller writes down next to a file it has already saved.
        for (const [i, file] of streamed.entries()) {
            expect(file.sha256).toBe(result.summary.shards[i].sha256);
            expect(file.name).toBe(result.summary.shards[i].file);
        }
        result.release();
    });

    /** A sink that throws is not survivable: the bytes have already left wasm, so the run fails as
     *  `io` rather than finish a set with a hole in it. */
    it("fails the run when the file sink throws, instead of finishing a set with a hole in it", async () => {
        const seen: string[] = [];
        await expect(
            assembleCells(cells(), sidecar, skin, { ...OPTIONS, forceSplit: true }, undefined, [], undefined, (file) => {
                seen.push(file.name);
                throw new Error("the card is full");
            }),
        ).rejects.toMatchObject({ code: "io" });
        expect(seen).toHaveLength(1);
    });

    /**
     * **The B2 determinism pin.** The same fixture, with the cells never copied into wasm memory —
     * handed over as identities and read a block at a time through a callback, the way the browser
     * reads them out of OPFS through a `FileSystemSyncAccessHandle`.
     *
     * Node has no OPFS, and that is the point of the seam: back the keys with buffers here and the
     * *engine's* path is identical to the browser's. If this produces the CLI's bytes, the streamed
     * path is not a different assembler.
     */
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
            undefined,
            store.sources(),
        );
        const want = expected("expected");
        expect(result.files.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of result.files.entries()) expectSameBytes(file.take(), want[i].bytes, file.name);
        // Every cell really came through the callback — a path that quietly found the bytes some
        // other way would pass the comparison above and prove nothing.
        expect(new Set(store.calls.map((c) => c.slot)).size).toBe(cells().length);
        result.release();
    });

    /** …and the volume set, where §2.3's 256 KiB verbatim copies (which bypass the cache) run beside
     *  §4.6.6's per-record emission (which is why it exists). */
    it("reproduces the native CLI's volume set from outside wasm memory too", async () => {
        const store = new Reads();
        const result = await assembleCells(
            [],
            sidecar,
            skin,
            { ...OPTIONS, forceSplit: true },
            undefined,
            [],
            terrain(),
            undefined,
            store.sources(),
        );
        const want = expected("expected-split");
        expect(result.files.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of result.files.entries()) expectSameBytes(file.take(), want[i].bytes, file.name);
        result.release();
    });

    /**
     * **What makes the seam affordable**, measured here rather than argued: `readBlockBytes: 1` is
     * the cache switched off, so its call count is exactly the number of reads the engine makes, and
     * the default's is what the host is actually asked for.
     *
     * The ratio is what matters at scale. §4.6.6 emits the merged edge pool one record at a time —
     * 17.5 M records at country scale (#1116 C3) — and every one of those, uncached, would be a JS
     * crossing (~0.4 µs, measured in Node by the PR that added this) *and* an OPFS syscall. Cached,
     * the host sees about one call per 64 KiB of cell.
     */
    it("serves the engine's reads from a block cache, without changing a byte", async () => {
        const run = async (readBlockBytes: number) => {
            const store = new Reads();
            const result = await assembleCells(
                [],
                sidecar,
                skin,
                { ...OPTIONS, readBlockBytes },
                undefined,
                [],
                undefined,
                undefined,
                store.sources(),
            );
            const files = result.files.map((f) => ({ name: f.name, bytes: f.take() }));
            result.release();
            return { files, calls: store.calls.length };
        };
        const uncached = await run(1);
        const cached = await run(64 * 1024);
        for (const [i, file] of uncached.files.entries()) {
            expectSameBytes(file.bytes, cached.files[i].bytes, `${file.name} with the read cache off`);
        }
        expect(cached.calls * 10).toBeLessThan(uncached.calls);
        // …and the reads it does make are whole blocks, not the engine's 30-byte records.
        expect(cached.calls).toBeGreaterThan(0);
    });

    /** A read that fails is `io` **naming the cell**, not a §4.8 `verify` defect and not a wasm trap:
     *  the browser's storage failed, and the message has to say which cell so a bug report can. */
    it("fails as io naming the cell when a read refuses", async () => {
        const store = new Reads();
        store.refuse = 1;
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, undefined, store.sources()),
        ).rejects.toMatchObject({ code: "io" });
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, undefined, store.sources()),
        ).rejects.toThrow(new RegExp(cells()[1].id.replace(/\//g, "/")));
    });

    /** A callback that throws is the same failure as one that refuses — it must not escape as an
     *  unclassified wasm exception, because the run's cleanup branches on the code. */
    it("fails as io when a read throws", async () => {
        const store = new Reads();
        store.throwAt = 0;
        await expect(
            assembleCells([], sidecar, skin, OPTIONS, undefined, [], undefined, undefined, store.sources()),
        ).rejects.toMatchObject({ code: "io" });
    });

    /**
     * Two runs in a row over the same cells, through one resolver. The browser's real reason for
     * this is that a `FileSystemSyncAccessHandle` is an exclusive lock — a run that leaks one makes
     * the *next* run fail to open the same cell (pinned against a modelled lock in
     * `../cells/store.test.ts`). Here it is the wasm side's half: no state from the first run — a
     * cached block, a slot table — may reach the second.
     */
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
                undefined,
                store.sources(),
            );
            const files = result.files.map((f) => ({ name: f.name, bytes: f.take() }));
            result.release();
            return files;
        };
        const first = await once();
        const second = await once();
        expect(second.map((f) => f.name)).toEqual(first.map((f) => f.name));
        for (const [i, file] of second.entries()) expectSameBytes(file.bytes, first[i].bytes, `${file.name}, run two`);
    });

    it("reports the §4.8 verify pass it already ran", async () => {
        const { summary } = await assembleCells(cells(), sidecar, skin, OPTIONS);
        expect(summary.cells).toBe(5);
        expect(summary.manifest).toBe("MS1.OBS");
        // A result the caller can hand to a device *because* the read-back happened in the tab.
        expect(summary.shards[0].verified?.chunks).toBeGreaterThan(0);
        expect(summary.shards[0].verified?.features).toBeGreaterThan(0);
        expect(summary.shards[0].sha256).toMatch(/^[0-9a-f]{64}$/);
    });

    /** EL4: the raster reaches the browser's output as its own file with the §5.2 derived name,
     *  and the §5.7 projection is the bytes actually written. */
    it("writes the terrain shard as its own file and prices it exactly", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS, undefined, [], terrain());
        const shard = result.files.find((f) => f.role === "terrain");
        expect(shard?.name).toBe("MS1.OBD");
        const bytes = shard!.take();
        expect(new TextDecoder().decode(bytes.subarray(0, 4))).toBe("OBCT");
        // 32-byte header + a 2 × 2 directory + three of four squares present.
        expect(bytes.length).toBe(32 + 16 + 3 * 2048);
        const t = result.summary.terrain as { bytes: number; cells: number; slots: number };
        expect(t.bytes).toBe(bytes.length);
        expect([t.cells, t.slots]).toEqual([3, 4]);
        result.release();
    });

    /** …and a selection with no raster is exactly the map it was before terrain existed (§13). */
    it("writes no terrain shard when the catalog publishes none", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        expect(result.files.some((f) => f.role === "terrain")).toBe(false);
        expect(result.summary.terrain).toBeNull();
        result.release();
    });

    it("reports every phase in order, never going backwards", async () => {
        const seen: { phase: AssemblePhase; fraction: number }[] = [];
        await assembleCells(cells(), sidecar, skin, OPTIONS, (phase, fraction) => {
            seen.push({ phase, fraction });
        });
        const order = seen.filter((s, i) => i === 0 || seen[i - 1].phase !== s.phase).map((s) => s.phase);
        expect(order).toEqual(["open", "poi", "nav", "plan", "write", "verify", "manifest", "done"]);
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

    /**
     * The §4.8 read-back is 74 % of a country-scale run and the engine makes one store call for the
     * whole of it, so this is the phase a cancel button is most likely to be pressed during — and
     * the one where "cancelled" must not be reported as `verify`, which the docs tell callers is a
     * defect never to retry past. Held from JS as well as from Rust because the wasm build is where
     * a caller actually presses it.
     */
    it("honours a cancel during the verify read-back, and calls it a cancellation", async () => {
        let seenVerify = 0;
        const attempt = assembleCells(cells(), sidecar, skin, OPTIONS, (phase) => {
            // Not the boundary callback — one from inside the read loop, which only the ByteSource
            // wrapper can honour.
            return phase === "verify" && ++seenVerify >= 3;
        });
        await expect(attempt).rejects.toMatchObject({ code: "aborted" });
        expect(seenVerify).toBe(3);
    });

    /**
     * A bar that reaches its maximum and then waits three fifths of the run is worse than no bar.
     * The wasm side is where that would be seen, so it is checked here too: the read-back reports
     * many times, forward, over a wide span.
     */
    it("keeps reporting through the §4.8 read-back instead of freezing", async () => {
        const verify: number[] = [];
        let beforeVerify = 0;
        await assembleCells(cells(), sidecar, skin, OPTIONS, (phase, fraction) => {
            if (phase === "verify") verify.push(fraction);
            else if (verify.length === 0) beforeVerify = fraction;
        });
        expect(beforeVerify).toBeLessThanOrEqual(0.41); // the write phase ends at 0.167 + 0.240 by weight
        expect(verify.length).toBeGreaterThanOrEqual(8);
        expect(verify.at(-1)! - verify[0]).toBeGreaterThan(0.3);
        for (let i = 1; i < verify.length; i++) expect(verify[i]).toBeGreaterThan(verify[i - 1]);
    });

    /**
     * A progress callback is the caller's own code and it can be broken. Losing a twenty-minute
     * assembly to a typo in a progress bar would be the worse failure, so it is not fatal — but it
     * is not silent either: a dead bar otherwise looks exactly like a hung assembler. One warning,
     * however many times it throws.
     */
    it("survives a progress callback that throws, and says so once", async () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
        try {
            let calls = 0;
            const result = await assembleCells(cells(), sidecar, skin, OPTIONS, () => {
                calls++;
                throw new Error("the progress bar is broken");
            });
            expect(calls).toBeGreaterThan(5); // it kept being called
            expect(result.files.at(-1)?.role).toBe("manifest"); // …and the assembly still finished
            expect(warn).toHaveBeenCalledTimes(1);
            expect(warn.mock.calls[0][0]).toMatch(/progress callback threw/);
            result.release();
        } finally {
            warn.mockRestore();
        }
    });

    /**
     * OBCA §4.8 makes the read-back a precondition of writing a set, and this bridge exists to hand
     * bytes to a device — so there is no way to ask for an unverified one. The Rust side pins that
     * the option is *parsed* leniently; this pins the behaviour a caller who tries it actually gets:
     * the key is ignored, not honoured, and the verify report is there in the summary.
     */
    it("ignores a skipVerify that a caller smuggles in", async () => {
        const sneaky = { ...OPTIONS, skipVerify: true, skip_verify: true } as Parameters<typeof assembleCells>[3];
        const { summary } = await assembleCells(cells(), sidecar, skin, sneaky);
        expect(summary.shards[0].verified?.chunks).toBeGreaterThan(0);
    });

    /**
     * The retry shape this refuses: take, upload, fail, take again. Returning an empty array the
     * second time writes a 0-byte shard to a card and reports success — a corrupt map that looks
     * like a working one.
     */
    it("refuses a second take() instead of handing back an empty file", async () => {
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
        const file = result.files[0];
        expect(file.take().length).toBe(file.byteLength);
        expect(() => file.take()).toThrow(AssembleError);
        try {
            file.take();
        } catch (e) {
            expect((e as AssembleError).code).toBe("internal");
            expect((e as AssembleError).message).toMatch(/already taken/);
        }
        // …and the size it reported before the take still reads true, so a caller planning a
        // transfer is not told the file is empty.
        expect(file.byteLength).toBeGreaterThan(0);
        result.release();
    });

    /** `release()` mid-iteration is the abandon path: the rest of the set stops being takeable, and
     *  releasing twice is not an error (a `finally` block may well do both). */
    it("makes the remaining files unavailable after a release mid-iteration", async () => {
        const result = await assembleCells(cells(), sidecar, skin, { ...OPTIONS, forceSplit: true });
        expect(result.files.length).toBe(4);
        result.files[0].take();
        result.release();
        for (const file of result.files.slice(1)) expect(() => file.take()).toThrow(AssembleError);
        expect(() => result.release()).not.toThrow();
    });

    /**
     * Two assemblies at once do not fit: each holds its inputs *and* its outputs in the same 4 GiB,
     * and one country-scale run already projects three quarters of it. The failure would not be a
     * catchable exception but the module aborting, so the second caller is refused up front —
     * synchronously, before the first `await`, or the guard would never see the overlap.
     */
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
        // §4.1: the coarse cell is `partial`, and the caller has to say so rather than discover it.
        await expect(assembleCells(cells(), sidecar, skin, { name: "x" })).rejects.toMatchObject({ code: "input" });

        // A corrupt download is a *format* problem, not a selection problem.
        const corrupt = cells();
        corrupt[0].bytes[0] ^= 0xff;
        await expect(assembleCells(corrupt, sidecar, skin, OPTIONS)).rejects.toMatchObject({ code: "format" });
    });

    it("surfaces the engine's own message, not a rewritten one", async () => {
        await expect(assembleCells(cells(), sidecar, skin, { name: "x" })).rejects.toThrow(/partial/);
    });
});

/**
 * The vocabularies that cross the wasm boundary as bare strings. The phase list is pinned by running
 * an assembly (above); the error codes cannot be — that would mean provoking all seven — so they are
 * pinned against the Rust source itself. A code renamed on one side only would turn every `catch`
 * that branches on it into a silent fall-through to `internal`.
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

describe("estimateMemory", () => {
    const MB = 1_000_000;
    /** The builder's split (`DownloadStep.svelte`'s `TARGET_SHARD_BYTES`). */
    const SHARD = 256 * 1024 * 1024;
    const STREAMED = { inputOnDisk: true, streamedShardBytes: SHARD };
    const DEVICE = { inputOnDisk: true, streamedShardBytes: 0 };

    it("passes a corridor and refuses DACH, before any download", async () => {
        const corridor = await estimateMemory(20 * MB, 60 * MB, 10 * MB, STREAMED);
        expect(corridor.fits).toBe(true);
        expect(corridor.headroomBytes).toBeGreaterThan(0);

        // DACH's engine term alone is past wasm32 — no residency mode argues with the merge itself.
        const dach = await estimateMemory(3000 * MB, 8500 * MB, 500 * MB, STREAMED);
        expect(dach.fits).toBe(false);
        expect(dach.engineBytes).toBeGreaterThan(dach.ceilingBytes);
        expect(dach.headroomBytes).toBeLessThan(0);
    });

    /**
     * The two output modes are genuinely different verdicts (#1116 B1): the download path streams
     * shards out and prices one of them; the device path keeps the set until `planned`. At
     * Bayern-ish scale (1.7× BW's published bytes) the first fits and the second does not — which
     * is why `DownloadStep` asks for both instead of gating everything on one.
     */
    it("prices the download and device paths apart, and they disagree at Bayern scale", async () => {
        const nav = 1.7 * 295_921_548;
        const cells = 1.7 * 853_456_890;
        const terrain = 1.7 * 58_721_264;
        const streamed = await estimateMemory(nav, cells, terrain, STREAMED);
        const device = await estimateMemory(nav, cells, terrain, DEVICE);
        expect(streamed.fits).toBe(true);
        expect(device.fits).toBe(false);
        expect(device.outputBytes).toBeGreaterThan(streamed.outputBytes);
        expect(device.engineBytes).toBe(streamed.engineBytes);
    });

    /** The buffered fallback — no usable OPFS — is the pre-B shape: full cells resident. */
    it("charges the whole selection when the input cannot stream", async () => {
        const cells = 717 * MB;
        const onDisk = await estimateMemory(271 * MB, cells, 0, STREAMED);
        const buffered = await estimateMemory(271 * MB, cells, 0, { inputOnDisk: false, streamedShardBytes: SHARD });
        expect(buffered.inputBytes).toBe(cells);
        // The streamed input is the cache plus terrain, not the selection.
        expect(onDisk.inputBytes).toBeLessThan(70 * MB);
        expect(buffered.peakBytes - onDisk.peakBytes).toBeGreaterThan(600 * MB);
    });

    /**
     * `fits` is a verdict against a desktop-shaped judgement (3 GiB of a 4 GiB address space), and a
     * phone's per-tab allowance is nowhere near it — so the number the verdict is measured against
     * is the caller's to lower. Nothing else moves: the projection is about the selection.
     */
    it("lets a caller lower the budget the verdict is measured against", async () => {
        const desktop = await estimateMemory(271 * MB, 717 * MB, 0, DEVICE);
        const phone = await estimateMemory(271 * MB, 717 * MB, 0, DEVICE, 1024 * 1024 * 1024);
        expect(desktop.fits).toBe(true);
        expect(phone.fits).toBe(false);
        expect(phone.peakBytes).toBe(desktop.peakBytes);
        expect(phone.budgetBytes).toBe(1024 * 1024 * 1024);
        expect(phone.ceilingBytes).toBe(desktop.ceilingBytes);
    });
});
