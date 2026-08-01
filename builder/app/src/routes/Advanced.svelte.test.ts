// @vitest-environment happy-dom

import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Preset, SchemaEnvelope } from "../lib/config/model";

const host = vi.hoisted(() => ({
    presets: vi.fn(),
    schema: vi.fn(),
    previewStatus: vi.fn(),
    previewPack: vi.fn(),
}));

vi.mock("../lib/platform", () => ({
    platform: {
        styleEditor: {
            presets: host.presets,
            schema: host.schema,
            preview: { status: host.previewStatus, pack: host.previewPack },
        },
    },
}));

import { working, type WorkingEnvelope } from "../lib/config/storage.svelte";
import Advanced from "./Advanced.svelte";

// Node 22 exposes an opt-in `localStorage` global that shadows happy-dom's
// storage unless the process receives a file flag. The route contract needs
// ordinary browser storage, so keep this test hermetic with the same API.
const storageValues = new Map<string, string>();
const memoryStorage: Storage = {
    get length() {
        return storageValues.size;
    },
    clear: () => storageValues.clear(),
    getItem: (key) => storageValues.get(key) ?? null,
    key: (index) => [...storageValues.keys()][index] ?? null,
    removeItem: (key) => storageValues.delete(key),
    setItem: (key, value) => storageValues.set(key, String(value)),
};
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: memoryStorage });

const schema: SchemaEnvelope = {
    schema_version: 1,
    format_version: 10,
    source: "binary",
    schema: { $defs: { style: { properties: {} } } },
};

const bikepacking: Preset = {
    id: "bikepacking",
    name: "Bikepacking",
    description: "The shipped schema",
    version: 3,
    config: {
        lods: [{ max_mpp: null, simplify: 0 }],
        features: {},
        marker: { color: "0xF800" },
    },
};

function existingEnvelope(): WorkingEnvelope {
    return {
        schema_version: 1,
        based_on: { id: "my-schema", version: 7 },
        modified: true,
        config: {
            lods: [{ max_mpp: null, simplify: 17 }],
            features: {},
            marker: { color: "0x1234" },
        },
        disabled: [],
    };
}

const mounted: ReturnType<typeof mount>[] = [];

function render() {
    const target = document.createElement("div");
    document.body.append(target);
    mounted.push(mount(Advanced, { target }));
    return target;
}

beforeEach(() => {
    localStorage.clear();
    working.envelope = null;
    host.presets.mockReset();
    host.schema.mockReset().mockResolvedValue(schema);
    host.previewStatus.mockReset().mockResolvedValue({
        available: false,
        label: "unavailable",
        configured: false,
        detail: "",
        bbox: "",
    });
    host.previewPack.mockReset();
    globalThis.fetch = vi.fn(async () => Response.json({ keys: {} })) as typeof fetch;
});

afterEach(async () => {
    while (mounted.length) await unmount(mounted.pop()!);
    working.envelope = null;
    localStorage.clear();
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

describe("Advanced first-use schema initialization", () => {
    it("copies the sole buildable maintainer preset on fresh storage", async () => {
        host.presets.mockResolvedValue([bikepacking]);
        const target = render();

        await vi.waitFor(() => {
            expect(working.envelope?.based_on).toEqual({ id: "bikepacking", version: 3 });
        });

        expect(working.envelope?.modified).toBe(false);
        expect(working.envelope?.config.marker.color).toBe("0xF800");
        expect(target.textContent).toContain("Preset: Bikepacking");
        expect(JSON.parse(localStorage.getItem("obcm.working") ?? "null").based_on).toEqual({
            id: "bikepacking",
            version: 3,
        });
    });

    it("restores and preserves an existing working config before presets load", async () => {
        const existing = existingEnvelope();
        localStorage.setItem("obcm.working", JSON.stringify(existing));
        host.presets.mockResolvedValue([bikepacking]);
        const target = render();

        await vi.waitFor(() => expect(host.presets).toHaveBeenCalledOnce());
        await vi.waitFor(() => expect(target.textContent).toContain("Custom — based on my-schema"));

        expect(working.envelope?.based_on).toEqual(existing.based_on);
        expect(working.envelope?.config.marker.color).toBe("0x1234");
        expect(JSON.parse(localStorage.getItem("obcm.working") ?? "null")).toEqual(existing);
    });

    it("does not replace an imported config while the preset request is in flight", async () => {
        let resolvePresets!: (presets: Preset[]) => void;
        host.presets.mockReturnValue(new Promise((resolve) => (resolvePresets = resolve)));
        const target = render();
        await vi.waitFor(() => expect(host.presets).toHaveBeenCalledOnce());

        const imported = existingEnvelope();
        working.adopt(imported);
        resolvePresets([bikepacking]);
        await vi.waitFor(() => expect(target.textContent).toContain("Custom — based on my-schema"));

        expect(working.envelope?.based_on).toEqual(imported.based_on);
        expect(working.envelope?.config.marker.color).toBe("0x1234");
    });

    it("does not choose arbitrarily when multiple buildable schemas are served", async () => {
        host.presets.mockResolvedValue([
            bikepacking,
            { ...bikepacking, id: "experimental", name: "Experimental", version: 1 },
        ]);
        const target = render();

        await vi.waitFor(() => {
            expect(target.textContent?.replace(/\s+/g, " ")).toContain(
                "Automatic first-use setup requires exactly one buildable schema preset, but this host provided 2.",
            );
        });
        expect(working.envelope).toBeNull();
    });
});
