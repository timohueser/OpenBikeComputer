import { describe, expect, it, vi } from 'vitest';

// map-style reads the page URL when it loads.
vi.stubGlobal('window', { location: { href: 'https://planner.example/plan/' } });
const { cropHeights, demTile, reliefZoom } = await import('./climate-terrain');
const { UNKNOWN_HEIGHT } = await import('./terrain');

const height = (col: number, row: number) => 10 * row + col - 100;
/** A 512 px DEM tile with a height at each (column, row). */
const dem = () => Int16Array.from({ length: 512 * 512 }, (_, i) => height(i % 512, Math.floor(i / 512)));

describe('climate terrain', () => {
    it('reads the DEM tile that the 512 px terrain source loads for the view', () => {
        expect(demTile(10, 533, 355)).toEqual({ z: 9, x: 266, y: 177, scale: 2, left: 256, top: 256 });
        expect(demTile(0, 0, 0)).toEqual({ z: 0, x: 0, y: 0, scale: 1, left: 0, top: 0 });
        // Above the DEM zoom the deepest DEM tile is cut.
        expect(demTile(15, 8 * 100 + 3, 8 * 50 + 7)).toEqual({ z: 12, x: 100, y: 50, scale: 8, left: 192, top: 448 });
    });

    it('reads a map point from the DEM zoom that the relief loads', () => {
        expect([reliefZoom(9.4), reliefZoom(9.6), reliefZoom(15), reliefZoom(-1)]).toEqual([9, 10, 12, 0]);
    });

    it('reads the heights of the covered part, one DEM pixel per map pixel at the view zoom', () => {
        const heights = dem();
        const quarter = cropHeights(heights, demTile(10, 533, 355));
        expect([quarter[0], quarter[1], quarter[256], quarter[65_535]]).toEqual([height(256, 256), height(257, 256), height(256, 257), height(511, 511)]);
        const whole = cropHeights(heights, demTile(0, 0, 0));
        expect([whole[1], whole[256]]).toEqual([height(2, 0), height(0, 2)]);
        const enlarged = cropHeights(heights, demTile(15, 803, 407));
        expect([enlarged[3], enlarged[4]]).toEqual([height(192, 448), height(193, 448)]);
        heights[0] = UNKNOWN_HEIGHT;
        expect(cropHeights(heights, demTile(0, 0, 0))[0]).toBeNaN();
    });
});
