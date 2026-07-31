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
    ])("%s GETs %s", async (_name, call, url) => {
        await call();
        expect(calls).toEqual([{ url, method: "GET" }]);
    });
});
