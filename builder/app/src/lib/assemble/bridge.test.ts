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
import { beforeAll, describe, expect, it } from "vitest";

import { AssembleError, assembleCells, estimateMemory, initAssemble } from "./bridge";
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
});
