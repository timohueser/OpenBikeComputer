// The desktop host's wire, pinned the way dev.wire.test.ts pins the dev host's.
//
// There is no HTTP here to read off a browser's network tab: an `invoke()` names
// a Rust function and hands it a serde-shaped argument object, and a mismatch
// surfaces as "command X not found" or a deserialization error at run time,
// inside a window, on someone else's machine. So the command names and argument
// shapes are asserted against what apps/obc-desktop/src/main.rs declares.

import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

/** The manifest the real generator writes — the same document web.wire.test.ts
 *  serves, so both hosts are tested against the producer, not a fixture. */
const EXAMPLE = readFileSync(
    new URL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url),
    "utf8",
);

interface Call {
    cmd: string;
    args: Record<string, unknown> | undefined;
}

const calls: Call[] = [];
let reply: (cmd: string) => unknown = () => ({});

/** The channel the build session opens; the test drives it like the backend
 *  would. Tauri's real one needs `window.__TAURI_INTERNALS__`. */
class FakeChannel<T> {
    onmessage: ((message: T) => void) | null = null;
}
const channels: FakeChannel<unknown>[] = [];

vi.mock("@tauri-apps/api/core", () => ({
    invoke: (cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args });
        return Promise.resolve(reply(cmd));
    },
    Channel: class extends FakeChannel<unknown> {
        constructor() {
            super();
            channels.push(this);
        }
    },
}));

const { platform } = await import("./desktop");

/** A fresh copy of the host, with its memoized catalog root reset — the
 *  catalog tests need one each, exactly like web.wire.test.ts's freshHost. */
async function freshHost() {
    vi.resetModules();
    return (await import("./desktop")).platform;
}

beforeEach(() => {
    calls.length = 0;
    channels.length = 0;
    reply = (cmd) => (cmd === "regions" ? { features: [] } : {});
});

describe("desktop host commands", () => {
    it.each([
        ["regions", () => platform.regions(), "regions"],
        ["presets", () => platform.presets(), "presets"],
        ["schema", () => platform.schema!(), "schema"],
        ["palette", () => platform.palette!(), "palette"],
    ])("%s invokes %s with no arguments", async (_name, call, cmd) => {
        await call();
        expect(calls).toEqual([{ cmd, args: undefined }]);
    });

    it("unwraps the region FeatureCollection, as the picker expects", async () => {
        await expect(platform.regions()).resolves.toEqual([]);
    });

    it("resolves catalog previews against the manifest's own URL", async () => {
        // §2: a preview reference resolves against the same base as an
        // artifact's `url`, and the manifest's location is that base — which is
        // why the command hands back the URL beside the body.
        reply = () => ({ url: "https://example.invalid/data/catalog.json", body: EXAMPLE });
        const catalog = await (await freshHost()).catalog();
        expect(catalog.schema_version).toBe(1);
        expect(catalog.presets[0].preview).toBe("https://example.invalid/data/previews/default.png");
    });

    it("refuses a malformed manifest rather than returning part of one", async () => {
        reply = () => ({ url: "https://example.invalid/catalog.json", body: '{"schema_version": 2}' });
        await expect((await freshHost()).catalog()).rejects.toThrow();
    });

    it("shares one catalog read between catalogRoot and catalog (#1041 A2)", async () => {
        // Envelope detection peeks at `catalogRoot`'s body and the chosen flow
        // parses the same body — so the two must come from one invoke, and a
        // second caller must not cost a second read.
        reply = () => ({ url: "https://example.invalid/data/catalog.json", body: EXAMPLE });
        const host = await freshHost();
        const root = await host.catalogRoot!();
        expect(root.body).toBe(EXAMPLE);
        await host.catalog();
        await host.catalogRoot!();
        expect(calls.map((c) => c.cmd)).toEqual(["catalog"]);
    });

    it("does not pin a refused body: after a parse refusal the next call re-reads", async () => {
        // Same rule as the web memo: a fulfilled read whose body the v1 parser
        // then refuses is un-pinned — it may be a v2 root the coverage flow
        // will accept, or a bad publish that gets fixed — so a retry re-reads
        // instead of re-refusing the same bytes.
        let body = "{oh no";
        reply = () => ({ url: "https://example.invalid/catalog.json", body });
        const host = await freshHost();
        await expect(host.catalog()).rejects.toThrow();
        body = EXAMPLE;
        await expect(host.catalog()).resolves.toMatchObject({ schema_version: 1 });
        expect(calls.map((c) => c.cmd)).toEqual(["catalog", "catalog"]);
    });

    it("starts a build with the request under `request` and a channel under `onEvent`", async () => {
        reply = () => "build-1";
        const session = platform.buildMap!();
        const req = {
            regionIds: ["europe/monaco"],
            config: { lods: [] },
            chunkSize: 4096,
            outputName: "mymap.obcm",
            bbox: [7.4, 43.7, 7.5, 43.8] as [number, number, number, number],
        };
        await session.start(req);
        expect(calls[0].cmd).toBe("build_start");
        // camelCase all the way through: the Rust side deserializes with
        // `#[serde(rename_all = "camelCase")]`, so unlike the dev host there is
        // no case hop to get wrong.
        expect(calls[0].args?.request).toEqual(req);
        expect(calls[0].args?.onEvent).toBe(channels[0]);
        expect(session.state).toBe("running");
    });

    it("turns the backend's events into the same state the dev host produces", async () => {
        reply = () => "build-1";
        const session = platform.buildMap!();
        await session.start({ regionIds: ["x"], config: {}, outputName: "m.obcm" });
        const channel = channels[0] as FakeChannel<Record<string, unknown>>;

        channel.onmessage?.({ type: "status", status: "converting", detail: "quadtree" });
        // "quadtree" is index 5 of 7 phases, over 8 slots.
        expect(session.pct).toBe(75);
        expect(session.phase).toBe("quadtree");

        channel.onmessage?.({ type: "log", line: "Building Quadtree LOD 0..." });
        expect(session.logLines).toEqual(["Building Quadtree LOD 0..."]);

        channel.onmessage?.({ type: "done", path: "/maps/m.obcm", filename: "m.obcm", size: 1024 });
        expect(session.state).toBe("done");
        // A path rather than a URL: the file is already on the disk it belongs on.
        expect(session.result).toEqual({ downloadUrl: "", filename: "m.obcm", size: 1024, path: "/maps/m.obcm" });
    });

    it("reports a cancelled build as cancelled, not as a failure", async () => {
        reply = () => "build-1";
        const session = platform.buildMap!();
        await session.start({ regionIds: ["x"], config: {}, outputName: "m.obcm" });
        await session.cancel!();
        expect(calls.at(-1)).toEqual({ cmd: "build_cancel", args: { id: "build-1" } });

        (channels[0] as FakeChannel<Record<string, unknown>>).onmessage?.({ type: "cancelled" });
        expect(session.state).toBe("cancelled");
        expect(session.error).toBeNull();
        expect(session.result).toBeNull();
    });

    it("does not cancel a build that never started", async () => {
        const session = platform.buildMap!();
        await session.cancel!();
        expect(calls).toEqual([]);
    });

    it("re-attaches to a build the window lost, replaying its log", async () => {
        reply = (cmd) => (cmd === "build_active" ? { id: "build-7", state: "running" } : true);
        const session = platform.buildMap!();
        await expect(session.reattach()).resolves.toBe(true);
        expect(calls[0]).toEqual({ cmd: "build_active", args: undefined });
        expect(calls[1].cmd).toBe("build_attach");
        expect(calls[1].args?.id).toBe("build-7");
        expect(session.state).toBe("running");
    });

    it("reattaches to nothing when no build is active", async () => {
        reply = () => null;
        await expect(platform.buildMap!().reattach()).resolves.toBe(false);
    });

    it("names a storage location by id, never by path", async () => {
        reply = () => 1234;
        await platform.storage!.clear("pbf");
        // The backend maps the id to a directory. A command that took a path
        // would be a delete-anything primitive reachable from a webview.
        expect(calls).toEqual([{ cmd: "storage_clear", args: { id: "pbf" } }]);
    });

    it("reveals a built map by its path", async () => {
        await platform.revealFile!("/maps/m.obcm");
        expect(calls).toEqual([{ cmd: "reveal_file", args: { path: "/maps/m.obcm" } }]);
    });
});
