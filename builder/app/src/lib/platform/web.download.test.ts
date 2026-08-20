import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { saveBlob } from "../download";

async function freshHost() {
    vi.resetModules();
    return (await import("./web")).platform;
}

beforeEach(() => {
    Object.assign(globalThis, { document: { baseURI: "https://maps.example.org/builder/" } });
});

afterEach(() => vi.unstubAllGlobals());

describe("web map delivery", () => {
    it("uses an ordinary download even when Chromium exposes a directory picker", async () => {
        const showDirectoryPicker = vi.fn();
        vi.stubGlobal("window", { showDirectoryPicker });

        const host = await freshHost();

        expect(host.openMapOutput).toBeNull();
        expect(showDirectoryPicker).not.toHaveBeenCalled();
    });

    it("dispatches an ordinary anchor download with the requested map name", () => {
        const click = vi.fn();
        const anchor = { href: "", download: "", click };
        const createObjectURL = vi.fn(() => "blob:assembled-map");
        vi.stubGlobal("URL", { createObjectURL });
        Object.assign(globalThis, {
            document: {
                baseURI: "https://maps.example.org/builder/",
                createElement: vi.fn(() => anchor),
            },
        });
        const blob = new Blob([Uint8Array.of(1, 2, 3, 4)]);

        saveBlob(blob, "Berlin.obcm");

        expect(createObjectURL).toHaveBeenCalledWith(blob);
        expect(anchor).toMatchObject({ href: "blob:assembled-map", download: "Berlin.obcm" });
        expect(click).toHaveBeenCalledOnce();
    });
});
