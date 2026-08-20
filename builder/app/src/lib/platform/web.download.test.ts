import { beforeEach, describe, expect, it, vi } from "vitest";

async function freshHost() {
    vi.resetModules();
    return (await import("./web")).platform;
}

beforeEach(() => {
    Object.assign(globalThis, { document: { baseURI: "https://maps.example.org/builder/" } });
});

describe("web map delivery", () => {
    it("uses an ordinary download even when Chromium exposes a directory picker", async () => {
        const showDirectoryPicker = vi.fn();
        vi.stubGlobal("window", { showDirectoryPicker });

        const host = await freshHost();

        expect(host.openMapOutput).toBeNull();
        expect(showDirectoryPicker).not.toHaveBeenCalled();
    });
});
