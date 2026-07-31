// The desktop host reads the same published catalog root as the website.

import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it, vi } from "vitest";

const EXAMPLE = readFileSync(
    new URL("../../../../../host/obc-pack/schema/catalog.example.json", import.meta.url),
    "utf8",
);

interface Call {
    cmd: string;
    args: unknown;
    options: unknown;
}

const calls: Call[] = [];
let reply: (cmd: string) => unknown = () => ({});

vi.mock("@tauri-apps/api/core", () => ({
    invoke: (cmd: string, args?: unknown, options?: unknown) => {
        calls.push({ cmd, args, options });
        return Promise.resolve(reply(cmd));
    },
    Channel: class { onmessage: ((message: unknown) => void) | null = null; },
}));

async function freshHost() {
    vi.resetModules();
    return (await import("./desktop")).platform;
}

beforeEach(() => {
    calls.length = 0;
    reply = () => ({});
});

describe("desktop catalog command", () => {
    it("returns the root exactly as Rust fetched it", async () => {
        reply = () => ({ url: "https://example.invalid/data/catalog.json", body: EXAMPLE });
        await expect((await freshHost()).catalog()).resolves.toEqual({
            url: "https://example.invalid/data/catalog.json",
            body: EXAMPLE,
        });
    });

    it("memoizes a fulfilled read", async () => {
        reply = () => ({ url: "https://example.invalid/catalog.json", body: EXAMPLE });
        const host = await freshHost();
        await host.catalog();
        await host.catalog();
        expect(calls.map((call) => call.cmd)).toEqual(["catalog"]);
    });

    it("retries a failed read", async () => {
        let fail = true;
        reply = () => {
            if (fail) throw new Error("offline");
            return { url: "https://example.invalid/catalog.json", body: EXAMPLE };
        };
        const host = await freshHost();
        await expect(host.catalog()).rejects.toThrow(/offline/);
        fail = false;
        await expect(host.catalog()).resolves.toMatchObject({ body: EXAMPLE });
        expect(calls.map((call) => call.cmd)).toEqual(["catalog", "catalog"]);
    });

    it("moves catalog objects through the native byte command", async () => {
        const bytes = new Uint8Array([1, 2, 3]);
        reply = (cmd) => (cmd === "catalog_get" ? bytes.buffer : {});
        const response = await (await freshHost()).catalogFetch("https://maps.example.test/cell.obcm");
        expect(new Uint8Array(await response.arrayBuffer())).toEqual(bytes);
        expect(calls.at(-1)).toMatchObject({
            cmd: "catalog_get",
            args: { url: "https://maps.example.test/cell.obcm" },
        });
    });

    it("groups assembled files in one native output session", async () => {
        reply = (cmd) => {
            if (cmd === "map_output_begin") return { id: 7, path: "/maps/Baden" };
            if (cmd === "map_output_write") return "/maps/Baden/MS1.OBS";
            return undefined;
        };
        const output = await (await freshHost()).openMapOutput!("Baden");
        const bytes = new Uint8Array([4, 5]);
        await expect(output.write("MS1.OBS", bytes)).resolves.toBe("/maps/Baden/MS1.OBS");
        await output.finish();
        expect(calls.map((call) => call.cmd)).toEqual(["map_output_begin", "map_output_write", "map_output_finish"]);
        expect(calls[1].args).toBe(bytes);
        expect(calls[1].options).toEqual({ headers: { "output-id": "7", filename: "MS1.OBS" } });
    });
});
