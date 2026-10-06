// The static host has no backend: its wire is the cell catalog root of the active
// planner release on a CDN. The catalog seam returns the root whole and leaves
// format validation to CatalogClient.

import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const EXAMPLE = readFileSync(
    new URL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url),
    "utf8",
);
const BASE = "https://maps.example.org/builder/";
const PLANNER = "https://maps.openbikecomputer.com/planner/catalog.json";
const release = (id: string) => `https://tiles.example.org/releases/${id}/device/catalog.json`;
const planner = (id: string) => JSON.stringify({ format: 1, active: { id, device_catalog: release(id) } });

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
    it("reads the cell catalog of the active planner release", async () => {
        globalThis.fetch = serve({ [PLANNER]: planner("a"), [release("a")]: EXAMPLE });
        const platform = await freshHost();
        await expect(platform.catalog()).resolves.toEqual({ url: release("a"), body: EXAMPLE });
        expect(urls).toEqual([PLANNER, release("a")]);
    });

    it("fetches the catalog once however many callers ask", async () => {
        globalThis.fetch = serve({ [PLANNER]: planner("a"), [release("a")]: EXAMPLE });
        const platform = await freshHost();
        await Promise.all([platform.catalog(), platform.catalog()]);
        expect(urls).toEqual([PLANNER, release("a")]);
    });

    it("reads the planner catalogue again once when the release is gone, and retries", async () => {
        const bodies: Record<string, string> = { [PLANNER]: planner("a") };
        const answer = serve(bodies);
        globalThis.fetch = (async (input: RequestInfo | URL) => {
            const response = await answer(input);
            bodies[PLANNER] = planner("b");
            bodies[release("b")] = EXAMPLE;
            return response;
        }) as typeof fetch;
        const platform = await freshHost();
        await expect(platform.catalog()).resolves.toEqual({ url: release("b"), body: EXAMPLE });
        expect(urls).toEqual([PLANNER, release("a"), PLANNER, release("b")]);
    });

    it("refreshes a fulfilled catalog and memoizes only the new complete release", async () => {
        const bodies = { [PLANNER]: planner("a"), [release("a")]: EXAMPLE, [release("b")]: EXAMPLE };
        globalThis.fetch = serve(bodies);
        const host = await freshHost();
        await host.catalog();
        bodies[PLANNER] = planner("b");
        await expect(host.catalog()).resolves.toMatchObject({ url: release("a") });
        await expect(host.catalog({ refresh: true })).resolves.toEqual({ url: release("b"), body: EXAMPLE });
        await host.catalog();
        expect(urls).toEqual([PLANNER, release("a"), PLANNER, release("b")]);
    });

    it("does not let an older failed read evict the refreshed catalog", async () => {
        let rejectOld!: (cause: Error) => void;
        globalThis.fetch = (() => new Promise<Response>((_, reject) => { rejectOld = reject; })) as typeof fetch;
        const host = await freshHost();
        const old = host.catalog();
        const refused = expect(old).rejects.toThrow(/unavailable/);
        globalThis.fetch = serve({ [PLANNER]: planner("b"), [release("b")]: EXAMPLE });
        const refreshed = await host.catalog({ refresh: true });
        rejectOld(new Error("offline"));
        await refused;
        await expect(host.catalog()).resolves.toBe(refreshed);
        expect(urls).toEqual([PLANNER, release("b")]);
    });

    it("does not pin a failed request", async () => {
        let status = 503;
        globalThis.fetch = (async (input: RequestInfo | URL) => {
            urls.push(String(input));
            return status === 200
                ? new Response(String(input) === PLANNER ? planner("a") : EXAMPLE, { status })
                : new Response("", { status, statusText: "Unavailable" });
        }) as typeof fetch;
        const platform = await freshHost();
        await expect(platform.catalog()).rejects.toThrow(/unavailable/);
        status = 200;
        await expect(platform.catalog()).resolves.toMatchObject({ body: EXAMPLE });
        expect(urls).toEqual([PLANNER, PLANNER, release("a")]);
    });
});
