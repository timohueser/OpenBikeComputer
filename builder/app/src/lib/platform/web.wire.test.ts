// The static host has no backend: its wire is the cell catalog root on a CDN.
// The catalog seam returns the root whole and leaves
// format validation to CatalogClient.

import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const EXAMPLE = readFileSync(
    new URL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url),
    "utf8",
);
const BASE = "https://maps.example.org/builder/";

let urls: string[];
const realFetch = globalThis.fetch;

function serve(bodies: Record<string, string>): typeof fetch {
    return (async (input: RequestInfo | URL) => {
        const url = String(input);
        urls.push(url);
        const body = bodies[url];
        return body === undefined
            ? new Response("", { status: 404, statusText: "Not Found" })
            : new Response(body, { status: 200 });
    }) as typeof fetch;
}

async function freshHost() {
    vi.resetModules();
    return (await import("./web")).platform;
}

beforeEach(() => {
    urls = [];
    Object.assign(globalThis, { document: { baseURI: BASE } });
});

afterEach(() => {
    globalThis.fetch = realFetch;
    vi.unstubAllGlobals();
});

describe("static documents", () => {
    it("reads the catalog from beside the app", async () => {
        globalThis.fetch = serve({
            [`${BASE}data/catalog.json`]: EXAMPLE,
        });
        const platform = await freshHost();
        await expect(platform.catalog()).resolves.toEqual({
            url: `${BASE}data/catalog.json`,
            body: EXAMPLE,
        });
        expect(urls).toEqual([`${BASE}data/catalog.json`]);
    });

    it("fetches the catalog once however many callers ask", async () => {
        globalThis.fetch = serve({ [`${BASE}data/catalog.json`]: EXAMPLE });
        const platform = await freshHost();
        await Promise.all([platform.catalog(), platform.catalog()]);
        expect(urls).toEqual([`${BASE}data/catalog.json`]);
    });

    it("does not pin a failed request", async () => {
        let status = 503;
        globalThis.fetch = (async (input: RequestInfo | URL) => {
            urls.push(String(input));
            return status === 200
                ? new Response(EXAMPLE, { status })
                : new Response("", { status, statusText: "Unavailable" });
        }) as typeof fetch;
        const platform = await freshHost();
        await expect(platform.catalog()).rejects.toThrow(/503/);
        status = 200;
        await expect(platform.catalog()).resolves.toMatchObject({ body: EXAMPLE });
        expect(urls).toHaveLength(2);
    });
});
