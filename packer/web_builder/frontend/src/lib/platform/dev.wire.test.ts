// The dev host is the one that has to be behaviour-identical after #895: local
// development goes on talking to `python -m packer.web_builder` exactly as it
// did. The seam introduced a camelCase→snake_case hop on the way into POST
// /jobs, which is precisely the kind of change that is invisible until a build
// 422s, so the requests are pinned here byte for byte.
//
// The expectations below were read off the pre-refactor client.ts and
// BuildCard.svelte; they are the wire, not this code's opinion of it.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { platform } from "./dev";

interface Recorded {
    url: string;
    method: string;
    headers: Record<string, string> | undefined;
    body: string | undefined;
}

let calls: Recorded[];
let streams: string[];
const realFetch = globalThis.fetch;

// The node test environment has neither, and the job tracker reaches for both
// the moment a build starts. Recording the stream URL makes it a fourth pinned
// request rather than merely a silenced crash.
const browserStubs = {
    EventSource: class {
        onmessage: unknown = null;
        onerror: unknown = null;
        constructor(url: string) {
            streams.push(url);
        }
        close() {}
    },
    sessionStorage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
};

beforeEach(() => {
    calls = [];
    streams = [];
    Object.assign(globalThis, browserStubs);
    globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
        calls.push({
            url: String(input),
            method: init?.method ?? "GET",
            headers: init?.headers as Record<string, string> | undefined,
            body: init?.body as string | undefined,
        });
        // Shapes chosen so each call's own post-processing runs for real.
        const payload = String(input).endsWith("/regions")
            ? { features: [] }
            : String(input).endsWith("/jobs")
              ? { job_id: "abc123" }
              : {};
        return new Response(JSON.stringify(payload), {
            status: 200,
            headers: { "Content-Type": "application/json" },
        });
    }) as typeof fetch;
});

afterEach(() => {
    globalThis.fetch = realFetch;
});

describe("dev host requests", () => {
    // The `!`s are this suite asserting what hosts.test.ts proves: the dev host
    // has every capability-gated member these five sit behind.
    it.each([
        ["regions", () => platform.regions(), "/api/regions"],
        ["presets", () => platform.presets(), "/api/presets"],
        ["schema", () => platform.schema!(), "/api/schema"],
        ["palette", () => platform.palette!(), "/api/palette"],
        ["legacyConfig", () => platform.legacyConfig!(), "/api/config/legacy"],
    ])("%s GETs %s", async (_name, call, url) => {
        await call();
        expect(calls).toEqual([{ url, method: "GET", headers: undefined, body: undefined }]);
    });

    it("unwraps the region FeatureCollection, as the picker expects", async () => {
        await expect(platform.regions()).resolves.toEqual([]);
    });

    it("POSTs a region build with the exact pre-refactor body", async () => {
        const session = platform.buildMap!();
        await session.start({
            regionIds: ["europe/monaco"],
            config: { lods: [] },
            chunkSize: 4096,
            outputName: "mymap.obcm",
        });
        expect(calls[0]).toEqual({
            url: "/api/jobs",
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: '{"region_ids":["europe/monaco"],"config":{"lods":[]},"chunk_size":4096,"output_name":"mymap.obcm"}',
        });
        // …and then follows the server's replayable event log.
        expect(streams).toEqual(["/api/jobs/abc123/events"]);
    });

    it("appends bbox last, and only in bbox mode", async () => {
        const session = platform.buildMap!();
        await session.start({
            regionIds: ["europe/monaco"],
            config: {},
            chunkSize: 8192,
            outputName: "box.obcm",
            bbox: [7.4, 43.7, 7.5, 43.8],
        });
        expect(calls[0].body).toBe(
            '{"region_ids":["europe/monaco"],"config":{},"chunk_size":8192,"output_name":"box.obcm","bbox":[7.4,43.7,7.5,43.8]}',
        );
    });
});
