import { describe, expect, it } from "vitest";
import { representativeMpp } from "./lods";

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
