import { describe, expect, it } from "vitest";
import { parseArticleIndex } from "./articles";
import { CatalogClient } from "./client";
import { planCells, downloadCells } from "./download";
import { cellSquare, parseCellId } from "./grid";
import { ledgerFor } from "./ledger";
import { parseRoot } from "./manifest";
import { resolveSelection } from "./selection";
import { EXAMPLE_ROOT, exampleCatalog, fixtureIndices } from "./testdata";

const id = "18/1204/1052";
const far = "18/1304/1152";
const rootUrl = "https://maps.example.org/maps/catalog.json";
const pin = { bytes: 3, sha256: "a".repeat(64), url: `/maps/objects/${"a".repeat(64)}` };
const document = () => ({ schema_version: exampleCatalog.schema_version, cells: [{ id, landmarks: pin }, { id: far, peaks: pin }] });

async function digest(body: string) {
    const bytes = new TextEncoder().encode(body);
    const hash = await crypto.subtle.digest("SHA-256", bytes);
    const sha256 = [...new Uint8Array(hash)].map((v) => v.toString(16).padStart(2, "0")).join("");
    return { bytes: bytes.length, sha256, url: `/maps/objects/${sha256}` };
}

function selection(catalog = exampleCatalog) {
    const indices = fixtureIndices(catalog, { network: [{ id, bytes: 19 }] });
    const box = cellSquare(parseCellId(id));
    const resolution = resolveSelection({ parts: [{ kind: "box", id: "pick", name: "Pick", box }], corridorRadiusM: 0 },
        { catalog, indices, regionCells: new Map() });
    return { resolution, indices };
}

describe("detached article catalog", () => {
    it("prices and downloads only artifacts on the selected network grid", async () => {
        const catalog = { ...exampleCatalog, terrain: null, articles: pin };
        const index = parseArticleIndex(JSON.stringify(document()), catalog);
        const { resolution, indices } = selection(catalog);
        const plan = planCells(resolution, catalog, indices, null, index);
        expect(plan.items.filter((item) => item.article)).toEqual([{ band: null, article: "landmarks", cell: { id, ...pin } }]);
        expect(plan.totalBytes).toBe(22);
        expect(ledgerFor(resolution, catalog, indices, index).totalBytes).toBe(22);
        expect(ledgerFor(resolution, catalog, indices).isFinal).toBe(false);
        expect(() => planCells(resolution, catalog, indices)).toThrow(/not been verified/);
        const body = "bin";
        const verified = await digest(body);
        const delivered: string[] = [];
        await downloadCells({ ...plan, items: [{ band: null, article: "landmarks", cell: { id, ...verified } }], totalBytes: 3 }, {
            fetchImpl: async () => new Response(body),
            onCell: (item, bytes) => { delivered.push(`${item.article}:${new TextDecoder().decode(bytes)}`); },
        });
        expect(delivered).toEqual(["landmarks:bin"]);
    });

    it.each([
        ["duplicate cell", (doc: ReturnType<typeof document>) => doc.cells.push(doc.cells[0])],
        ["wrong grid", (doc: ReturnType<typeof document>) => { doc.cells[0].id = "19/0602/0526"; }],
        ["noncanonical id", (doc: ReturnType<typeof document>) => { doc.cells[0].id = "18/01204/1052"; }],
        ["missing collections", (doc: ReturnType<typeof document>) => { delete (doc.cells[0] as { landmarks?: unknown }).landmarks; }],
        ["wrong URL digest", (doc: ReturnType<typeof document>) => { doc.cells[0].landmarks = { ...pin, url: `/maps/objects/${"b".repeat(64)}` }; }],
    ])("rejects %s", (_name, edit) => {
        const doc = document();
        edit(doc);
        expect(() => parseArticleIndex(JSON.stringify(doc), exampleCatalog)).toThrow();
    });

    it("keeps a failed pinned index as an error and retries before accepting sparse absence", async () => {
        const body = JSON.stringify({ schema_version: exampleCatalog.schema_version, cells: [] });
        const ref = await digest(body);
        const root = JSON.parse(EXAMPLE_ROOT);
        root.articles = ref;
        for (const region of root.regions) region.article_bytes = 0;
        let response = "bad";
        const client = CatalogClient.fromBody(JSON.stringify(root), rootUrl, { attempts: 1, fetchImpl: async () => new Response(response) });
        expect(parseRoot(JSON.stringify(root)).articles).toEqual(ref);
        await expect(client.articles()).rejects.toThrow();
        response = body;
        const index = await client.articles();
        expect(index?.cells).toEqual([]);
        expect(await client.articles()).toBe(index);
    });
});
