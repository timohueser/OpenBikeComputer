// The static host has no server to talk to, so "the wire" is two documents on
// a CDN. What's pinned here is where it looks for them, that it reads a
// manifest body whole before parsing it (OBCC §7), and that a refused manifest
// doesn't wedge the session.
//
// Each test re-imports the module, because the host memoizes both documents for
// the life of a page load — which is the behaviour being asserted in one of
// them.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";

const EXAMPLE = readFileSync(
    new URL("../../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url),
    "utf8",
);

const BASE = "https://maps.example.org/builder/";

let urls: string[];
const realFetch = globalThis.fetch;

/** Serve the two static documents; anything else is a 404. */
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

/** A fresh copy of the host, with its memoized documents reset. */
async function freshHost() {
    vi.resetModules();
    return (await import("./web")).platform;
}

beforeEach(() => {
    urls = [];
    // The host resolves its relative defaults against the page, so the page is
    // the one browser fact these tests have to stand in for.
    Object.assign(globalThis, { document: { baseURI: BASE } });
});

afterEach(() => {
    globalThis.fetch = realFetch;
    vi.unstubAllGlobals();
});

describe("static documents", () => {
    it("reads the regions and the catalog from beside the app", async () => {
        globalThis.fetch = serve({
            [`${BASE}data/regions.json`]: JSON.stringify({ features: [] }),
            [`${BASE}data/catalog.json`]: EXAMPLE,
        });
        const platform = await freshHost();
        await expect(platform.regions()).resolves.toEqual([]);
        await platform.catalog();
        expect(urls).toEqual([`${BASE}data/regions.json`, `${BASE}data/catalog.json`]);
    });

    it("fetches each document once, however many callers ask", async () => {
        globalThis.fetch = serve({ [`${BASE}data/catalog.json`]: EXAMPLE });
        const platform = await freshHost();
        // The picker draws the coverage, the store joins it, the preset list
        // reads its styles: one document, one request.
        await Promise.all([platform.catalog(), platform.catalog(), platform.presets()]);
        expect(urls).toEqual([`${BASE}data/catalog.json`]);
    });
});

describe("the catalog seam", () => {
    it("hands back the whole parsed manifest", async () => {
        globalThis.fetch = serve({ [`${BASE}data/catalog.json`]: EXAMPLE });
        const catalog = await (await freshHost()).catalog();
        expect(catalog.schema_version).toBe(1);
        expect(catalog.artifacts).toHaveLength(5);
    });

    it("rejects a malformed manifest rather than returning part of one", async () => {
        globalThis.fetch = serve({
            [`${BASE}data/catalog.json`]: JSON.stringify({ schema_version: 99 }),
        });
        // Matched on the message, not the class: `vi.resetModules()` gives each
        // fresh host its own copy of the manifest module, so its error class is
        // not the one this file imported.
        await expect((await freshHost()).catalog()).rejects.toThrow(/schema_version 99/);
    });

    it("does not pin a failure: a later call tries again", async () => {
        // A memoized rejection would make one bad response permanent until the
        // tab is reloaded, and the catalog store's retry pointless.
        let body = "{oh no";
        globalThis.fetch = (async (input: RequestInfo | URL) => {
            urls.push(String(input));
            return new Response(body, { status: 200 });
        }) as typeof fetch;
        const platform = await freshHost();
        await expect(platform.catalog()).rejects.toThrow(/not a JSON document/);
        body = EXAMPLE;
        await expect(platform.catalog()).resolves.toMatchObject({ schema_version: 1 });
        expect(urls).toHaveLength(2);
    });
});

describe("the preset seam", () => {
    it("is the manifest's own preset list, with no packer config to carry", async () => {
        globalThis.fetch = serve({ [`${BASE}data/catalog.json`]: EXAMPLE });
        const presets = await (await freshHost()).presets();
        expect(presets.map((p) => p.id)).toEqual(["default", "minimal"]);
        expect(presets[0].name).toBe("Bikepacking");
        expect(presets[0].config).toBeUndefined();
    });

    it("resolves a preview reference against the manifest's own URL", async () => {
        // §2: a preview resolves against the base the artifacts are published
        // under, and the manifest's location is that base.
        globalThis.fetch = serve({ [`${BASE}data/catalog.json`]: EXAMPLE });
        const presets = await (await freshHost()).presets();
        expect(presets[0].preview).toBe(`${BASE}data/previews/default.png`);
        expect(presets[1].preview).toBeUndefined();
    });
});
