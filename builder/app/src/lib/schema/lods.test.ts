import { describe, expect, it } from "vitest";
import { previewLadder, representativeMpp } from "./lods";

describe("schema preview LOD scales", () => {
    const lods = [null, 400, 120, 30, 16, 10, 5, 3, 1.2].map((max_mpp) => ({ max_mpp, simplify: 0 }));

    it("covers the complete shipped ladder at exact production thresholds", () => {
        expect(lods.map((_lod, index) => representativeMpp(lods, index))).toEqual([
            480, 400, 120, 30, 16, 10, 5, 3, 1.2,
        ]);
    });

    it("keeps malformed custom ladders within the renderer bridge bounds", () => {
        const malformed = [{ max_mpp: null, simplify: 0 }, { max_mpp: null, simplify: 0 }];
        expect(representativeMpp(malformed, 0)).toBe(40);
        expect(representativeMpp(malformed, 1)).toBeGreaterThanOrEqual(0.5);
    });

    it("does not hide a valid custom coarse threshold behind a display-only clamp", () => {
        const custom = [{ max_mpp: null, simplify: 0 }, { max_mpp: 500, simplify: 0 }];
        expect(representativeMpp(custom, 0)).toBe(600);
    });
});

describe("previewLadder", () => {
    const editing = [null, 400, 120, 30].map((max_mpp) => ({ max_mpp, simplify: 0 }));
    const packed = [null, 30, 16].map((max_mpp) => ({ max_mpp, simplify: 0 }));

    it("shows the editable ladder before anything is packed", () => {
        expect(previewLadder(null, editing)).toEqual(editing);
    });

    it("shows the ladder the DISPLAYED map was packed with, not the one being edited", () => {
        // Adding two far-zoom tiers must not put two chips on screen that the loaded map has no
        // tiers for — clicking one would only dispatch the renderer to some other LOD.
        expect(previewLadder(packed, editing, 3)).toEqual(packed);
    });

    it("lets the loaded map's own table decide how many chips there are", () => {
        const trimmed = previewLadder(packed, editing, 2);
        expect(trimmed).toHaveLength(2);
        expect(trimmed[1].max_mpp).toBe(30);
        // And a tier the map carries that neither ladder describes still gets a chip.
        expect(previewLadder(packed, editing, 4)).toHaveLength(4);
    });
});
