// The client, against the real documents.
//
// `catalog.example.json` pins its satellites by digest, and the two satellite
// examples checked in beside it *are* those bytes — verified here rather than
// assumed, since the whole §9 guarantee is that pin. So this suite serves the
// real files over a fake fetch and lets the real SHA-256 decide, which means a
// producer that regenerates one example and not the others fails here.

import { describe, expect, it, vi } from "vitest";
import { BytesVerificationError } from "../download";
import { CatalogFormatError } from "./manifest";
import { CatalogClient } from "./client";
import { EXAMPLE_CELL_INDEX, EXAMPLE_REGION_CELLS, EXAMPLE_ROOT, exampleCatalog } from "./testdata";

const ROOT_URL = "https://maps.example.org/catalog/catalog.json";
const FINE_INDEX_URL = new URL(exampleCatalog.cell_index.find((ref) => ref.band === "fine")!.url, ROOT_URL).href;
const SWISS_CELLS_URL = new URL(
    exampleCatalog.regions.find((region) => region.id === "europe/switzerland")!.cells_url,
    ROOT_URL,
).href;
const DEFAULT_PREVIEW_URL = new URL(
    exampleCatalog.skins.find((skin) => skin.id === "default")!.preview!.url,
    ROOT_URL,
).href;
const FIRST_FINE_CELL_URL = (JSON.parse(EXAMPLE_CELL_INDEX) as { cells: Array<{ url: string }> }).cells[0].url;
const DEFAULT_PREVIEW = "example preview png";

async function sha256Hex(body: string): Promise<string> {
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(body));
    return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A fetch over a fixed URL → body map, counting requests. */
function serving(bodies: Record<string, string>) {
    const calls: string[] = [];
    const impl = vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        calls.push(url);
        const body = bodies[url];
        if (body === undefined) return new Response("no such object", { status: 404, statusText: "Not Found" });
        return new Response(new TextEncoder().encode(body), { status: 200 });
    }) as unknown as typeof fetch;
    return { impl, calls };
}

function allBodies(over: Record<string, string> = {}): Record<string, string> {
    return {
        [ROOT_URL]: EXAMPLE_ROOT,
        [FINE_INDEX_URL]: EXAMPLE_CELL_INDEX,
        [SWISS_CELLS_URL]: EXAMPLE_REGION_CELLS,
        [DEFAULT_PREVIEW_URL]: DEFAULT_PREVIEW,
        ...over,
    };
}

describe("CatalogClient.load", () => {
    it("reads and validates the root", async () => {
        const { impl } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        expect(client.catalog.schema.id).toBe("bikepacking");
        expect(client.baseUrl).toBe(ROOT_URL);
    });

    it("refuses a relative URL rather than guessing a base", async () => {
        // Resolving one is a host's job: the web host has a `document.baseURI`,
        // the desktop host does not, and a client that guessed would resolve a
        // CDN path against a Tauri asset scheme.
        await expect(CatalogClient.load("./data/catalog.json", { fetchImpl: serving({}).impl })).rejects.toThrow(
            CatalogFormatError,
        );
    });

    it("surfaces an HTTP failure as itself", async () => {
        const { impl } = serving({});
        await expect(CatalogClient.load(ROOT_URL, { fetchImpl: impl })).rejects.toThrow(/404/);
    });

    it("rejects a root of another envelope", async () => {
        const { impl } = serving({ [ROOT_URL]: '{"schema_version": 1, "presets": [], "artifacts": []}' });
        await expect(CatalogClient.load(ROOT_URL, { fetchImpl: impl })).rejects.toThrow(CatalogFormatError);
    });
});

describe("CatalogClient.fromBody", () => {
    it("builds a client from a body already in hand, fetching nothing", async () => {
        // Envelope detection's whole point (#1038): the root was fetched once
        // to be peeked at, and constructing the client must not fetch it again.
        const { impl, calls } = serving(allBodies());
        const client = CatalogClient.fromBody(EXAMPLE_ROOT, ROOT_URL, { fetchImpl: impl });
        expect(calls).toHaveLength(0);
        expect(client.catalog.schema.id).toBe("bikepacking");
        // …and the satellites still resolve against where the body came from.
        const fine = await client.cellIndex("fine");
        expect(fine.cells[0].url).toBe(FIRST_FINE_CELL_URL);
    });

    it("refuses a relative URL exactly as load does", () => {
        expect(() => CatalogClient.fromBody(EXAMPLE_ROOT, "./data/catalog.json")).toThrow(CatalogFormatError);
    });

    it("rejects an invalid body whole, like any other parse", () => {
        expect(() => CatalogClient.fromBody('{"schema_version": 1}', ROOT_URL)).toThrow(CatalogFormatError);
    });
});

describe("cellIndex", () => {
    it("verifies the satellite against the root's pin and parses it", async () => {
        const { impl } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        const fine = await client.cellIndex("fine");
        expect(fine.cells.map((c) => c.id)).toEqual(["18/1204/1052", "18/1204/1053"]);
        // Cell URLs are resolved once, here, so nothing downstream holds a
        // reference that means different things depending on where it is read.
        expect(fine.cells[0].url).toBe(FIRST_FINE_CELL_URL);
    });

    it("rejects a satellite whose bytes are not the ones the root hashed", async () => {
        // A byte the digest did not cover — the failure §9 exists to catch.
        const tampered = EXAMPLE_CELL_INDEX.replace('"bytes": 552', '"bytes": 553');
        const { impl } = serving(allBodies({ [FINE_INDEX_URL]: tampered }));
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.cellIndex("fine")).rejects.toThrow(BytesVerificationError);
        // …and the root is retained rather than patched: the client is still
        // usable, and a retry is a retry rather than a reload.
        expect(client.catalog.schema.id).toBe("bikepacking");
    });

    it("rejects a satellite of the wrong length before hashing it", async () => {
        const { impl } = serving(allBodies({ [FINE_INDEX_URL]: EXAMPLE_CELL_INDEX + " " }));
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.cellIndex("fine")).rejects.toThrow(/longer than the catalog/);
    });

    it("fetches each satellite once, and pins nothing on failure", async () => {
        const bodies = allBodies();
        const { impl, calls } = serving(bodies);
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await Promise.all([client.cellIndex("fine"), client.cellIndex("fine")]);
        expect(calls.filter((u) => u === FINE_INDEX_URL)).toHaveLength(1);

        // A CDN hiccup must not make a band permanently unavailable for the life
        // of the page, so a rejected load is not the cached one.
        const failing = serving(allBodies({ [FINE_INDEX_URL]: "not json" }));
        const client2 = await CatalogClient.load(ROOT_URL, { fetchImpl: failing.impl });
        await expect(client2.cellIndex("fine")).rejects.toThrow();
        await expect(client2.cellIndex("fine")).rejects.toThrow();
        expect(failing.calls.filter((u) => u === FINE_INDEX_URL)).toHaveLength(2);
    });

    it("refuses a satellite that is not text, without letting the decoder guess", async () => {
        // The digest can only prove the bytes are the ones the root hashed. It
        // cannot prove they are a document — a mis-served object, a truncated
        // gzip, a CDN error page in some other encoding all hash to something.
        // The decoder is `fatal: true` precisely so this arrives as "not a
        // catalog" rather than as a JSON parse error about `�`.
        const notText = new Uint8Array([0x7b, 0xff, 0xfe, 0x7d]);
        const digest = await crypto.subtle.digest("SHA-256", notText as unknown as BufferSource);
        const root = JSON.parse(EXAMPLE_ROOT);
        const ref = root.cell_index.find((r: { band: string }) => r.band === "fine");
        ref.bytes = notText.byteLength;
        ref.sha256 = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
        ref.url = ref.url.replace(/[0-9a-f]{64}(?=\.json$)/, ref.sha256);

        const impl = vi.fn(async (input: RequestInfo | URL) => {
            if (String(input) === ROOT_URL) return new Response(JSON.stringify(root));
            if (String(input) === ref.url) return new Response(notText as unknown as BodyInit);
            return new Response("no such object", { status: 404, statusText: "Not Found" });
        }) as unknown as typeof fetch;

        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.cellIndex("fine")).rejects.toThrow(CatalogFormatError);
        await expect(client.cellIndex("fine")).rejects.toThrow(/not valid UTF-8/);
    });

    it("has nothing to say about a band the schema does not have", async () => {
        const { impl } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.cellIndex("vivid")).rejects.toThrow(/no cell index/);
    });
});

describe("skinPreview", () => {
    it("verifies and memoises the optional PNG bytes", async () => {
        const { impl, calls } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        const [first, second] = await Promise.all([client.skinPreview("default"), client.skinPreview("default")]);
        expect(new TextDecoder().decode(first!)).toBe(DEFAULT_PREVIEW);
        expect(second).toBe(first);
        expect(calls.filter((url) => url === DEFAULT_PREVIEW_URL)).toHaveLength(1);
        expect(await client.skinPreview("contrast")).toBeNull();
    });

    it("refuses preview bytes that do not match the root pin", async () => {
        const { impl } = serving(allBodies({ [DEFAULT_PREVIEW_URL]: `${DEFAULT_PREVIEW}!` }));
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.skinPreview("default")).rejects.toThrow(BytesVerificationError);
    });

    it("rejects a skin the catalog does not offer", async () => {
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: serving(allBodies()).impl });
        await expect(client.skinPreview("moonlight")).rejects.toThrow(/no skin/);
    });
});

describe("regionCellList", () => {
    it("verifies, parses, and cross-checks against the indices already in hand", async () => {
        const { impl } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await client.cellIndex("fine");
        const cells = await client.regionCellList("europe/switzerland");
        expect(cells.cells.fine).toEqual(["18/1204/1052", "18/1204/1053", "18/1204/1055"]);
    });

    it("catches a region naming a cell its band's index does not have", async () => {
        // §6's cross-document MUST, end to end: a bakery that published a
        // region list one cell ahead of the index it points into. Both documents
        // are internally valid and correctly pinned — the root is re-pinned here
        // so they are — which is precisely why the check has to be a third one.
        // Not a hole, either: a hole is ground with no cell, this is a named
        // cell with no bytes, size or digest.
        const thinned = JSON.parse(EXAMPLE_CELL_INDEX);
        thinned.cells.pop();
        const body = JSON.stringify(thinned);
        const root = JSON.parse(EXAMPLE_ROOT);
        const ref = root.cell_index.find((r: { band: string }) => r.band === "fine");
        ref.cell_count = 1;
        ref.bytes = new TextEncoder().encode(body).byteLength;
        ref.sha256 = await sha256Hex(body);
        ref.url = ref.url.replace(/[0-9a-f]{64}(?=\.json$)/, ref.sha256);

        const { impl } = serving(allBodies({ [ROOT_URL]: JSON.stringify(root), [ref.url]: body }));
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await client.cellIndex("fine");
        await expect(client.regionCellList("europe/switzerland")).rejects.toThrow(/18\/1204\/1053/);
    });

    it("has nothing to say about a region the catalog does not have", async () => {
        const { impl } = serving(allBodies());
        const client = await CatalogClient.load(ROOT_URL, { fetchImpl: impl });
        await expect(client.regionCellList("europe/atlantis")).rejects.toThrow(/no region/);
    });
});
