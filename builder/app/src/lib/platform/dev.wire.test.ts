// The maintainer dev host keeps only the schema-editor HTTP seam. Product map
// building consumes the same published catalog as web and desktop.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { platform } from "./dev";

interface Recorded {
    url: string;
    method: string;
}

let calls: Recorded[];
const realFetch = globalThis.fetch;

beforeEach(() => {
    calls = [];
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        calls.push({ url: String(input), method: init?.method ?? "GET" });
        return new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch;
});

afterEach(() => {
    globalThis.fetch = realFetch;
});

describe("dev host requests", () => {
    it.each([
        ["presets", () => platform.presets(), "/api/presets"],
        ["schema", () => platform.schema!(), "/api/schema"],
        ["palette", () => platform.palette!(), "/api/palette"],
        ["legacyConfig", () => platform.legacyConfig!(), "/api/config/legacy"],
        ["preview status", () => platform.schemaPreview!.status(), "/api/schema-preview/status"],
    ])("%s GETs %s", async (_name, call, url) => {
        await call();
        expect(calls).toEqual([{ url, method: "GET" }]);
    });

    it("reads the configured published catalog at runtime instead of baking it into Vite", async () => {
        globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
            const url = String(input);
            calls.push({ url, method: init?.method ?? "GET" });
            if (url === "/api/runtime") {
                return new Response(JSON.stringify({ catalog_url: "https://maps.example.test/live/catalog.json" }), {
                    status: 200,
                    headers: { "Content-Type": "application/json" },
                });
            }
            return new Response("catalog-body", { status: 200 });
        }) as typeof fetch;

        await expect(platform.catalog()).resolves.toEqual({
            url: "https://maps.example.test/live/catalog.json",
            body: "catalog-body",
        });
        expect(calls).toEqual([
            { url: "/api/runtime", method: "GET" },
            { url: "https://maps.example.test/live/catalog.json", method: "GET" },
        ]);
    });

    it("posts only config JSON and propagates cancellation", async () => {
        const controller = new AbortController();
        globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
            calls.push({ url: String(input), method: init?.method ?? "GET" });
            expect(init?.body).toBe(JSON.stringify({ lods: [], features: {} }));
            expect(init?.signal).toBe(controller.signal);
            return new Response(new Uint8Array([79, 66, 67, 77]), {
                status: 200,
                headers: { "X-OBC-Pack-Duration-Ms": "42" },
            });
        }) as typeof fetch;

        await expect(
            platform.schemaPreview!.pack({ lods: [], features: {} }, controller.signal),
        ).resolves.toEqual({ bytes: new Uint8Array([79, 66, 67, 77]), packDurationMs: 42 });
        expect(calls).toEqual([{ url: "/api/schema-preview", method: "POST" }]);
    });
});
