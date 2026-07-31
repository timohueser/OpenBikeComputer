import { describe, expect, it } from "vitest";
import { cellId, GRID_ORIGIN } from "../catalog/v2/grid";
import { exampleCatalog } from "../catalog/v2/testdata";
import { degreesToUbox, detailBandId, parseCells, patchCount, ringToDegrees, uboxToDegrees } from "./shape";

describe("detailBandId", () => {
    it("picks the finest geometry band of the real example schema", () => {
        // coarse 2^20 (context), mid 2^19, fine 2^18, network 2^18 (core, no
        // LODs). The outline everyone sees is the fine band's.
        expect(detailBandId(exampleCatalog)).toBe("fine");
    });
});

describe("patchCount", () => {
    it("counts a solid block as one patch", () => {
        expect(patchCount([cellId(18, 100, 100), cellId(18, 100, 101), cellId(18, 101, 100), cellId(18, 101, 101)])).toBe(1);
    });

    it("counts disjoint patches — the corridor panel's gaps are this minus one", () => {
        expect(patchCount([cellId(18, 100, 100), cellId(18, 200, 200)])).toBe(2);
    });

    it("does not count a hole as a patch", () => {
        // A 3×3 ring with the middle missing: one patch, two rings.
        const cells = [];
        for (let i = 0; i < 3; i++) {
            for (let j = 0; j < 3; j++) {
                if (i === 1 && j === 1) continue;
                cells.push(cellId(18, 100 + i, 100 + j));
            }
        }
        expect(patchCount(cells)).toBe(1);
    });
});

describe("coordinate adapters", () => {
    it("round-trips a drawn box outward through µdeg and back", () => {
        const box = degreesToUbox(47.1234567, 7.1, 47.9, 8.2);
        // Rounded outward: the box never shrinks past what was drawn.
        expect(box.minLat).toBeLessThanOrEqual(47.1234567 * 1e6);
        expect(box.maxLat).toBeGreaterThanOrEqual(47.9 * 1e6);
        const [[s, w], [n, e]] = uboxToDegrees(box);
        expect(s).toBeLessThanOrEqual(47.1234567);
        expect(w).toBeLessThanOrEqual(7.1);
        expect(n).toBeGreaterThanOrEqual(47.9);
        expect(e).toBeGreaterThanOrEqual(8.2);
    });

    it("turns rings into Leaflet's [lat, lon] degrees", () => {
        expect(ringToDegrees([[47_500_000, 7_250_000]])).toEqual([[47.5, 7.25]]);
    });

    it("parses canonical ids into cells", () => {
        const [cell] = parseCells(["18/1204/1052"]);
        expect(cell.log2).toBe(18);
        expect(GRID_ORIGIN + cell.i * 2 ** 18).toBe(47_185_920);
    });
});
