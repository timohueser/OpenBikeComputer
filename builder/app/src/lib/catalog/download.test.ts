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

/** A response whose body arrives one chunk at a time, `delayMs` apart — so a
 *  test can put one slot's progress reports firmly after another slot's
 *  failure instead of hoping. */
function slowStream(chunks: Uint8Array[], delayMs: number): Response {
    return new Response(
        new ReadableStream({
            async start(controller) {
                for (const chunk of chunks) {
                    await new Promise((r) => setTimeout(r, delayMs));
                    controller.enqueue(chunk);
                }
                controller.close();
            },
        }) as unknown as BodyInit,
    );
}

/** Serves the body a cell's URL implies, so the digest matches by construction. */
function serving(over: Record<string, Uint8Array> = {}, onFetch?: () => void) {
    return vi.fn(async (input: RequestInfo | URL) => {
        onFetch?.();
        const url = String(input);
        const override = over[url];
        if (override) return new Response(override as unknown as BodyInit, { status: 200 });
        // …/cells/<band>/<i>/<j>.<sha256>.obcm → "<band>/<log2>/<i>/<j>"
        const m = /\/cells\/([a-z]+)\/(\d+)\/(\d+)\.[0-9a-f]{64}\.obcm$/.exec(url);
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

    it("does not download a known-empty cell", async () => {
        const { indices: artifactIndices } = await fixtures();
        const indices = new Map(artifactIndices);
        indices.set(
            "fine",
            fixtureIndices(
                exampleCatalog,
                {
                    fine: IDS.fine.map((id) => artifactIndices.get("fine")!.byId.get(id)!).map((cell) => ({
                        id: cell.id,
                        bytes: cell.bytes,
                        sha256: cell.sha256,
                    })),
                },
                { fine: [{ start: "18/1204/1055", end: "18/1204/1055" }] },
            ).get("fine")!,
        );
        const empty = cellSquare(parseCellId("18/1204/1055"));
        const part: BoxPart = {
            kind: "box",
            id: "empty",
            name: "Known empty",
            box: {
                minLat: empty.minLat + 1,
                minLon: empty.minLon + 1,
                maxLat: empty.maxLat - 1,
                maxLon: empty.maxLon - 1,
            },
        };
        const resolution = resolveSelection(
            { parts: [part], corridorRadiusM: 0 },
            { catalog: exampleCatalog, indices, regionCells: new Map() },
        );
        const plan = planCells(resolution, exampleCatalog, indices);
        expect(resolution.cellsByBand.get("fine")).toEqual(["18/1204/1055"]);
        expect(resolution.missingByBand.get("fine")).toBeUndefined();
        expect(plan.items.some((item) => item.band === "fine")).toBe(false);
        expect(plan.knownEmpty).toContainEqual({ band: "fine", id: "18/1204/1055" });
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
        // A wrong body that is also *shorter* is indistinguishable from a torn
        // one, so it earns the retries before it is refused. The sleep is stubbed
        // rather than the attempts pinned, so the refusal still has to survive
        // them.
        await expect(
            downloadCells(plan, { fetchImpl: impl, sleep: () => Promise.resolve(), onCell: () => {} }),
        ).rejects.toThrow(BytesVerificationError);
    });

    it("survives a connection dropped mid-cell, which is the failure that ends real runs", async () => {
        // The one observed on 2026-08-09: the CDN closes the connection part-way
        // through a multi-megabyte cell, the reader ends clean and short, and a
        // run of hundreds of cells throws away everything it had. Retrying is
        // safe because the object is digest-pinned — the second attempt cannot
        // slip past the check that the first one failed.
        const { plan } = await fixtures();
        const victim = plan.items[2];
        const whole = bodyFor("fine/18/1204/1052");
        let seen = 0;
        const impl = serving({}, () => {}) as unknown as typeof fetch;
        const dropping = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
            if (String(input) === victim.cell.url && seen++ < 2) {
                // Half the body, then a clean close — a torn download.
                return new Response(whole.slice(0, whole.byteLength >> 1) as unknown as BodyInit);
            }
            return impl(input, init);
        }) as unknown as typeof fetch;

        const delivered: string[] = [];
        const result = await downloadCells(plan, {
            fetchImpl: dropping,
            concurrency: 1,
            sleep: () => Promise.resolve(),
            onCell: (item) => void delivered.push(item.cell.id),
        });
        expect(seen).toBe(3); // two torn bodies, then the whole one
        expect(result.cells).toBe(plan.items.length);
        expect(delivered).toHaveLength(plan.items.length);
    });

    it("gives up on a cell the origin simply does not have, without retrying it", async () => {
        // A 404 is the catalog and the store disagreeing. Another attempt neither
        // fixes that nor tells the rider anything they did not already know.
        const { plan } = await fixtures();
        const victim = plan.items[1];
        let hits = 0;
        const base = serving({}) as unknown as typeof fetch;
        const missing = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
            if (String(input) === victim.cell.url) {
                hits += 1;
                return new Response("nope", { status: 404, statusText: "Not Found" });
            }
            return base(input, init);
        }) as unknown as typeof fetch;
        await expect(
            downloadCells(plan, {
                fetchImpl: missing,
                concurrency: 1,
                sleep: () => Promise.resolve(),
                onCell: () => {},
            }),
        ).rejects.toThrow(/404/);
        expect(hits).toBe(1);
    });

    it("stops the rest of the run on the first failure", async () => {
        const { plan } = await fixtures();
        let started = 0;
        const impl = serving({ [plan.items[0].cell.url]: new TextEncoder().encode("wrong") }, () => {
            started += 1;
        });
        await expect(
            // `attempts: 1` — this is about the *plan* stopping, so the retry
            // that a torn body would otherwise earn is not in the way of counting
            // how many cells were started.
            downloadCells(plan, { fetchImpl: impl, concurrency: 1, attempts: 1, onCell: () => {} }),
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

    it("stops mid-run when the caller aborts, and delivers nothing after that", async () => {
        // The realistic abort: the rider changes the selection, or closes the
        // dialog, halfway through 1 000 cells. Every cell delivered after that
        // point is a write into an assembly nobody is waiting for.
        const { plan } = await fixtures();
        const controller = new AbortController();
        const delivered: string[] = [];
        const run = downloadCells(plan, {
            fetchImpl: serving(),
            concurrency: 1,
            signal: controller.signal,
            onCell: async (item) => {
                delivered.push(item.cell.id);
                if (delivered.length === 2) controller.abort(new Error("the rider changed their mind"));
                await new Promise((r) => setTimeout(r, 1));
            },
        });
        await expect(run).rejects.toThrow(/changed their mind/);
        expect(delivered).toHaveLength(2);
        expect(plan.items.length).toBeGreaterThan(2);
    });

    it("delivers nothing more once a cell has failed", async () => {
        // Concurrency 2 with a bad cell in the first pair: the *other* slot's
        // cell arrives verified after the failure, and it must not be written
        // anywhere. A sink is usually a file, or a wasm assembler, and neither
        // enjoys a write after the rejection it already reported.
        const { plan } = await fixtures();
        const first = plan.items[0].cell.url;
        const second = plan.items[1].cell;
        const impl = vi.fn(async (input: RequestInfo | URL) => {
            const url = String(input);
            if (url === first) return new Response(new TextEncoder().encode("wrong") as unknown as BodyInit);
            if (url === second.url) {
                // Arrives well after the failure has been reported.
                return slowStream([bodyFor(`mid/19/0602/0526`)], 15);
            }
            return new Response("nope", { status: 404, statusText: "Not Found" });
        }) as unknown as typeof fetch;
        const delivered: string[] = [];
        await expect(
            downloadCells(plan, {
                fetchImpl: impl,
                concurrency: 2,
                // The failure has to land while the other slot's body is still
                // arriving; a retried short body would move it past that.
                attempts: 1,
                onCell: (item) => void delivered.push(item.cell.id),
            }),
        ).rejects.toThrow(BytesVerificationError);
        // …and still nothing once the late body has finished arriving.
        await new Promise((r) => setTimeout(r, 60));
        expect(delivered).toEqual([]);
    });

    it("does not keep counting a body that never arrived", async () => {
        // A cell that fails mid-body leaves its partial bytes in the in-flight
        // table, and every later report from another slot adds them again — a
        // bar that creeps past what was received, for bytes that will never
        // come. The other slot's body is deliberately slow here so its reports
        // land after the failure and would carry the leak.
        const { plan } = await fixtures();
        const first = plan.items[0].cell.url;
        const secondBody = bodyFor(`mid/19/0602/0526`);
        const truncated = 10;
        const impl = vi.fn(async (input: RequestInfo | URL) => {
            const url = String(input);
            if (url === first) return slowStream([bodyFor(`coarse/20/0301/0263`).slice(0, truncated)], 1);
            if (url === plan.items[1].cell.url) {
                return slowStream([secondBody.slice(0, 20), secondBody.slice(20)], 15);
            }
            return new Response("nope", { status: 404, statusText: "Not Found" });
        }) as unknown as typeof fetch;
        const reports: number[] = [];
        await expect(
            downloadCells(plan, {
                fetchImpl: impl,
                concurrency: 2,
                // One attempt, so the truncated slot's partial bytes are released
                // at the same point in the run this test was written around.
                attempts: 1,
                onCell: () => {},
                onProgress: (p) => reports.push(p.receivedBytes),
            }),
        ).rejects.toThrow(BytesVerificationError);
        await new Promise((r) => setTimeout(r, 60));
        // Nothing ever reported more than the one body that was really being
        // received. With the truncated cell's ten bytes still counted, every
        // report after it would be ten too high.
        expect(Math.max(...reports)).toBe(secondBody.byteLength);
        expect(reports.some((r) => r > truncated && r <= secondBody.byteLength)).toBe(true);
    });

    it("has nothing to do for an empty plan", async () => {
        const impl = serving();
        expect(await downloadCells({ items: [], knownEmpty: [], totalBytes: 0, terrainBytes: 0 }, { fetchImpl: impl, onCell: () => {} })).toEqual({
            cells: 0,
            bytes: 0,
        });
        expect(impl).not.toHaveBeenCalled();
    });
});
