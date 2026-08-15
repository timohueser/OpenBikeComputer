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
    const editor = platform.styleEditor!;

    it.each([
        ["presets", () => editor.presets(), "/api/presets"],
        ["schema", () => editor.schema(), "/api/schema"],
        ["palette", () => editor.palette(), "/api/palette"],
        ["preview status", () => editor.preview.status(), "/api/schema-preview/status"],
    ])("%s GETs %s", async (_name, call, url) => {
        await call();
        expect(calls).toEqual([{ url, method: "GET" }]);
    });

    it("reads the configured published catalog through the runtime host instead of baking it into Vite", async () => {
        globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
            const url = String(input);
            calls.push({ url, method: init?.method ?? "GET" });
            return new Response("catalog-body", {
                status: 200,
                headers: { "X-OBC-Catalog-Url": "https://maps.example.test/live/catalog.json" },
            });
        }) as typeof fetch;

        await expect(platform.catalog()).resolves.toEqual({
            url: "https://maps.example.test/live/catalog.json",
            body: "catalog-body",
        });
        expect(calls).toEqual([{ url: "/api/catalog/root", method: "GET" }]);
    });

    it("moves satellites and cells through the same-origin bounded proxy", async () => {
        globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
            calls.push({ url: String(input), method: init?.method ?? "GET" });
            return new Response(new Uint8Array([1, 2, 3]), { status: 200 });
        }) as typeof fetch;
        const object = "https://maps.example.test/live/cells/12/3/4.obcm";
        const response = await platform.catalogFetch(object);
        expect(new Uint8Array(await response.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]));
        expect(calls).toEqual([{
            url: `/api/catalog/object?url=${encodeURIComponent(object)}`,
            method: "GET",
        }]);
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
            editor.preview.pack({ lods: [], features: {} }, controller.signal),
        ).resolves.toEqual({ bytes: new Uint8Array([79, 66, 67, 77]), packDurationMs: 42, diagnostics: [] });
        expect(calls).toEqual([{ url: "/api/schema-preview", method: "POST" }]);
    });
});
