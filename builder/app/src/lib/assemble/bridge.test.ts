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
import type { AssembleCell, AssemblePhase } from "./bridge";

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

/**
 * What the native CLI left in `tests/fixture/<dir>/`, in the order the bridge must hand it on:
 * shards ascending, the OBCS manifest last (OBCA §5.4).
 */
function expected(dir: string): { name: string; bytes: Uint8Array }[] {
    const root = join(FIXTURE, dir);
    const files = readdirSync(root).map((name) => ({ name, bytes: new Uint8Array(readFileSync(join(root, name))) }));
    files.sort(
        (a, b) => Number(a.name.endsWith(".OBS")) - Number(b.name.endsWith(".OBS")) || a.name.localeCompare(b.name),
    );
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
        const result = await assembleCells(cells(), sidecar, skin, OPTIONS);
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
        const result = await assembleCells(cells(), sidecar, skin, { ...OPTIONS, forceSplit: true });
        const want = expected("expected-split");
        expect(result.files.map((f) => f.role)).toEqual(["core", "coarse", "geometry", "manifest"]);
        expect(result.files.map((f) => f.name)).toEqual(want.map((f) => f.name));
        for (const [i, file] of result.files.entries()) {
            expectSameBytes(file.take(), want[i].bytes, file.name);
        }
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
     * A bar that reaches its maximum and then waits three quarters of the run is worse than no bar.
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
        expect(beforeVerify).toBeLessThanOrEqual(0.32); // the write phase ends at 0.318 by weight
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
    it("passes a corridor and refuses DACH, before any download", async () => {
        const MB = 1_000_000;
        const corridor = await estimateMemory(20 * MB, 60 * MB);
        expect(corridor.fits).toBe(true);
        expect(corridor.headroomBytes).toBeGreaterThan(0);

        // PR #1027's own DACH projection: nav ≈ 11.5× switzerland's 271 MB.
        const dach = await estimateMemory(11.5 * 271 * MB, 11.5 * 717 * MB);
        expect(dach.fits).toBe(false);
        expect(dach.peakBytes).toBeGreaterThan(dach.ceilingBytes);
        expect(dach.headroomBytes).toBeLessThan(0);
    });

    /**
     * `fits` is a verdict against a desktop-shaped judgement (3 GiB of a 4 GiB address space), and a
     * phone's per-tab allowance is nowhere near it — so the number the verdict is measured against
     * is the caller's to lower. Nothing else moves: the projection is about the selection.
     */
    it("lets a caller lower the budget the verdict is measured against", async () => {
        const MB = 1_000_000;
        const desktop = await estimateMemory(271 * MB, 717 * MB);
        const phone = await estimateMemory(271 * MB, 717 * MB, 1024 * 1024 * 1024);
        expect(desktop.fits).toBe(true);
        expect(phone.fits).toBe(false);
        expect(phone.peakBytes).toBe(desktop.peakBytes);
        expect(phone.budgetBytes).toBe(1024 * 1024 * 1024);
        expect(phone.ceilingBytes).toBe(desktop.ceilingBytes);
    });
});
