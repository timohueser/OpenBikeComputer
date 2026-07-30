// Fetching a cell set: the plan, the pool, and the verification that does not
// soften just because there are now hundreds of objects instead of one.
//
// The digests here are real — the bodies are hashed with node's WebCrypto, the
// same `crypto.subtle` a browser gives us — so the check is exercised end to end
// rather than stubbed into agreement.

import { describe, expect, it, vi } from "vitest";
import { BytesVerificationError } from "../download";
import { downloadCells, planCells, type CellDownloadItem } from "./download";
import { cellSquare, parseCellId } from "./grid";
import { resolveSelection, type BoxPart, type SelectionContext } from "./selection";
import { exampleCatalog, fixtureIndices } from "./testdata";

async function sha256Hex(bytes: Uint8Array): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", bytes as unknown as BufferSource);
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A cell's bytes, deterministic per id so the digest is checkable. */
function bodyFor(id: string): Uint8Array {
    return new TextEncoder().encode(`obcm cell ${id} — not really a map, but bytes are bytes`);
}

const IDS = {
    coarse: ["20/0301/0263"],
    mid: ["19/0602/0526"],
    fine: ["18/1204/1052", "18/1204/1053"],
    network: ["18/1204/1052", "18/1204/1053"],
};

async function fixtures() {
    const spec: Record<string, { id: string; bytes: number; sha256: string }[]> = {};
    for (const [band, ids] of Object.entries(IDS)) {
        spec[band] = await Promise.all(
            ids.map(async (id) => {
                const body = bodyFor(`${band}/${id}`);
                return { id, bytes: body.byteLength, sha256: await sha256Hex(body) };
            }),
        );
    }
    const indices = fixtureIndices(exampleCatalog, spec);
    const A = cellSquare(parseCellId("18/1204/1052"));
    const B = cellSquare(parseCellId("18/1204/1053"));
    const part: BoxPart = {
        kind: "box",
        id: "box",
        name: "Both",
        box: { minLat: A.minLat + 1, minLon: A.minLon + 1, maxLat: A.maxLat - 1, maxLon: B.maxLon - 1 },
    };
    const ctx: SelectionContext = { catalog: exampleCatalog, indices, regionCells: new Map() };
    const resolution = resolveSelection({ parts: [part], corridorRadiusM: 0 }, ctx);
    return { indices, plan: planCells(resolution, exampleCatalog, indices) };
}

/** Serves the body a cell's URL implies, so the digest matches by construction. */
function serving(over: Record<string, Uint8Array> = {}, onFetch?: () => void) {
    return vi.fn(async (input: RequestInfo | URL) => {
        onFetch?.();
        const url = String(input);
        const override = over[url];
        if (override) return new Response(override as unknown as BodyInit, { status: 200 });
        // …/cells/<band>/<i>/<j>.obcm → "<band>/<log2>/<i>/<j>"
        const m = /\/cells\/([a-z]+)\/(\d+)\/(\d+)\.obcm$/.exec(url);
        if (!m) return new Response("nope", { status: 404, statusText: "Not Found" });
        const log2 = m[1] === "coarse" ? 20 : m[1] === "mid" ? 19 : 18;
        return new Response(bodyFor(`${m[1]}/${log2}/${m[2]}/${m[3]}`) as unknown as BodyInit, { status: 200 });
    }) as unknown as typeof fetch;
}

describe("planCells", () => {
    it("orders by schema band, then canonical cell id, and totals the bytes", async () => {
        const { plan } = await fixtures();
        expect(plan.items.map((i) => `${i.band} ${i.cell.id}`)).toEqual([
            "coarse 20/0301/0263",
            "mid 19/0602/0526",
            "fine 18/1204/1052",
            "fine 18/1204/1053",
            "network 18/1204/1052",
            "network 18/1204/1053",
        ]);
        expect(plan.totalBytes).toBe(plan.items.reduce((sum, i) => sum + i.cell.bytes, 0));
    });

    it("leaves holes out of the plan without calling them an error", async () => {
        // A missing cell is an empty leaf and the renderer paints backdrop
        // there; the ledger already reported it as coverage the rider accepted.
        const { indices } = await fixtures();
        const A = cellSquare(parseCellId("18/1204/1052"));
        const westOfA: BoxPart = {
            kind: "box",
            id: "w",
            name: "w",
            box: { minLat: A.minLat + 1, minLon: A.minLon - 1, maxLat: A.maxLat - 1, maxLon: A.minLon + 1 },
        };
        const resolution = resolveSelection(
            { parts: [westOfA], corridorRadiusM: 0 },
            { catalog: exampleCatalog, indices, regionCells: new Map() },
        );
        const plan = planCells(resolution, exampleCatalog, indices);
        expect(resolution.missingByBand.get("fine")).toEqual(["18/1204/1051"]);
        expect(plan.items.map((i) => i.cell.id)).not.toContain("18/1204/1051");
    });
});

describe("downloadCells", () => {
    it("verifies and delivers every cell", async () => {
        const { plan } = await fixtures();
        const got: string[] = [];
        const summary = await downloadCells(plan, {
            fetchImpl: serving(),
            onCell: (item: CellDownloadItem, bytes) => {
                got.push(`${item.band} ${item.cell.id}`);
                expect(bytes.byteLength).toBe(item.cell.bytes);
            },
        });
        expect(summary).toEqual({ cells: plan.items.length, bytes: plan.totalBytes });
        expect(got.sort()).toEqual(plan.items.map((i) => `${i.band} ${i.cell.id}`).sort());
    });

    it("aggregates progress across the whole set, ending at the known total", async () => {
        const { plan } = await fixtures();
        const seen: number[] = [];
        await downloadCells(plan, {
            fetchImpl: serving(),
            concurrency: 2,
            onCell: () => {},
            onProgress: (p) => {
                expect(p.totalBytes).toBe(plan.totalBytes);
                expect(p.totalCells).toBe(plan.items.length);
                seen.push(p.receivedBytes);
            },
        });
        expect(seen.at(-1)).toBe(plan.totalBytes);
        // Monotone: an in-flight body's bytes are never double-counted when it
        // completes.
        expect(seen).toEqual([...seen].sort((a, b) => a - b));
    });

    it("keeps at most `concurrency` cells in flight", async () => {
        const { plan } = await fixtures();
        let active = 0;
        let peak = 0;
        const impl = serving({}, () => {
            active += 1;
            peak = Math.max(peak, active);
        });
        await downloadCells(plan, {
            fetchImpl: impl,
            concurrency: 2,
            onCell: async () => {
                await new Promise((r) => setTimeout(r, 1));
                active -= 1;
            },
        });
        expect(peak).toBe(2);
        expect(active).toBe(0);
    });

    it("refuses a cell whose bytes are not the ones the catalog hashed", async () => {
        const { plan } = await fixtures();
        const victim = plan.items[3];
        const impl = serving({ [victim.cell.url]: new TextEncoder().encode("something else entirely") });
        await expect(downloadCells(plan, { fetchImpl: impl, onCell: () => {} })).rejects.toThrow(
            BytesVerificationError,
        );
    });

    it("stops the rest of the run on the first failure", async () => {
        const { plan } = await fixtures();
        let started = 0;
        const impl = serving({ [plan.items[0].cell.url]: new TextEncoder().encode("wrong") }, () => {
            started += 1;
        });
        await expect(
            downloadCells(plan, { fetchImpl: impl, concurrency: 1, onCell: () => {} }),
        ).rejects.toThrow();
        // Concurrency 1 and the first item bad: nothing after it is attempted.
        expect(started).toBe(1);
    });

    it("honours an abort signal", async () => {
        const { plan } = await fixtures();
        const controller = new AbortController();
        controller.abort();
        await expect(
            downloadCells(plan, { fetchImpl: serving(), signal: controller.signal, onCell: () => {} }),
        ).rejects.toThrow();
    });

    it("has nothing to do for an empty plan", async () => {
        const impl = serving();
        expect(await downloadCells({ items: [], totalBytes: 0 }, { fetchImpl: impl, onCell: () => {} })).toEqual({
            cells: 0,
            bytes: 0,
        });
        expect(impl).not.toHaveBeenCalled();
    });
});
