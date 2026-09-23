import { describe, expect, it, vi } from "vitest";

import type { SkinEntry } from "../catalog/manifest";
import { openLiveSkinPreview, renderSkinPreviewFrames, type LiveSkinPreview } from "./preview";
import { canonicalSchema, canonicalSkin } from "./testdata";
import { cloneSkin } from "./custom";

const bridge = vi.hoisted(() => ({ open: vi.fn(), setStyles: vi.fn(), setTheme: vi.fn() }));
vi.mock("./pkg/obc_skin_preview.js", () => ({
    default: vi.fn(),
    SkinPreview: class {
        width = 2;
        height = 2;
        constructor() { bridge.open(); }
        set_styles = bridge.setStyles;
        set_theme = bridge.setTheme;
    },
}));

function skin(id: string): SkinEntry {
    return { id } as SkinEntry;
}

function fakePreview(): LiveSkinPreview & { shared: Uint8ClampedArray } {
    const shared = new Uint8ClampedArray(2 * 2 * 4);
    let skin = "";
    return {
        width: 2,
        height: 2,
        shared,
        setStyles(next) {
            skin = next;
            shared.fill(next.includes('"id":"blue"') ? 0x22 : 0x11);
        },
        setTheme: vi.fn(),
        panBy: vi.fn(),
        zoomAt: vi.fn(),
        resetCamera: vi.fn(),
        stats: vi.fn(() => ({
            metersPerPixel: 5,
            lodIndex: 4,
            lodCount: 7,
            featuresDrawn: 1,
            featuresDropped: 0,
            pointsDrawn: 2,
            spanUtilization: 0,
            pointUtilization: 0,
            ringUtilization: 0,
        })),
        frame: () => {
            if (!skin) throw new Error("skin was not applied");
            return shared;
        },
        free: vi.fn(),
    };
}

describe("renderSkinPreviewFrames", () => {
    it("restamps one resident production bridge and owns each card frame", async () => {
        const preview = fakePreview();
        const open = vi.fn(async () => preview);
        const frames = await renderSkinPreviewFrames("schema", [skin("red"), skin("blue")], {
            open,
            yieldToBrowser: async () => {},
        });

        expect(open).toHaveBeenCalledTimes(1);
        expect(preview.free).toHaveBeenCalledTimes(1);
        expect([...frames.red.pixels]).toEqual(Array(16).fill(0x11));
        expect([...frames.blue.pixels]).toEqual(Array(16).fill(0x22));
        preview.shared.fill(0xff);
        expect(frames.red.pixels[0], "cards must not retain wasm's transient frame view").toBe(0x11);
    });

    it("yields between cards and stops a stale render after cancellation", async () => {
        const preview = fakePreview();
        const controller = new AbortController();
        let yields = 0;
        const frames = await renderSkinPreviewFrames("schema", [skin("red"), skin("blue")], {
            open: async () => preview,
            signal: controller.signal,
            yieldToBrowser: async () => {
                yields++;
                controller.abort();
            },
        });
        expect(Object.keys(frames)).toEqual(["red"]);
        expect(yields).toBe(1);
        expect(preview.free).toHaveBeenCalledTimes(1);
    });

    it("does not open wasm for no custom skins and frees it after a render failure", async () => {
        const openEmpty = vi.fn();
        await expect(renderSkinPreviewFrames("schema", [], { open: openEmpty })).resolves.toEqual({});
        expect(openEmpty).not.toHaveBeenCalled();

        const preview = fakePreview();
        preview.setStyles = () => {
            throw new Error("bad saved skin");
        };
        await expect(renderSkinPreviewFrames("schema", [skin("bad")], { open: async () => preview })).rejects.toThrow(
            /bad saved skin/,
        );
        expect(preview.free).toHaveBeenCalledTimes(1);
    });
});


describe("live preview skin admission", () => {
    it("rejects an invalid initial draft before opening the renderer or fetching a map", async () => {
        bridge.open.mockClear();
        const fetchImpl = vi.fn();
        const draft = cloneSkin(canonicalSkin);
        draft.styles.find((style) => style.feature_type === "highway.primary")!.z_index = 128;
        await expect(openLiveSkinPreview(JSON.stringify(canonicalSchema), JSON.stringify(draft), JSON.stringify(canonicalSkin), { fetchImpl }))
            .rejects.toThrow(/drawing order/i);
        expect(bridge.open).not.toHaveBeenCalled();
        expect(fetchImpl).not.toHaveBeenCalled();
        for (const unknown of [{ ...canonicalSchema, id: "unknown" }, { ...canonicalSchema, revision: 2 }]) {
            await expect(openLiveSkinPreview(JSON.stringify(unknown), JSON.stringify(canonicalSkin), JSON.stringify(canonicalSkin), { fetchImpl }))
                .rejects.toThrow(/unavailable for this map schema/);
        }
        expect(bridge.open).not.toHaveBeenCalled();
        expect(fetchImpl).not.toHaveBeenCalled();
    });

    it("keeps the last accepted skin when a drawing order is out of range", async () => {
        bridge.setStyles.mockClear();
        const preview = await openLiveSkinPreview(JSON.stringify({ schema: canonicalSchema }), JSON.stringify(canonicalSkin), JSON.stringify(canonicalSkin), { map: new Uint8Array() });
        const draft = cloneSkin(canonicalSkin);
        const water = draft.styles.find((style) => style.feature_type === "natural.water")!;
        water.z_index = 16;
        preview.setStyles(JSON.stringify(draft), JSON.stringify(canonicalSkin));
        expect(bridge.setStyles).toHaveBeenCalledTimes(1);
        for (const z of [-129, 128, 1.5]) {
            water.z_index = z;
            expect(() => preview.setStyles(JSON.stringify(draft), JSON.stringify(canonicalSkin))).toThrow(/drawing order/i);
        }
        expect(bridge.setStyles).toHaveBeenCalledTimes(1);
        water.z_index = -128;
        preview.setStyles(JSON.stringify(draft), JSON.stringify(canonicalSkin));
        expect(bridge.setStyles).toHaveBeenCalledTimes(2);
    });
});
