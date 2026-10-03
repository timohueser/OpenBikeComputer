import { describe, expect, it } from 'vitest';
import { cropHeights, demTile } from './climate-terrain';

/** A 512 px Terrarium tile whose height at (column, row) is 10 × row + column − 100. */
function terrarium(): Uint8ClampedArray {
    const rgba = new Uint8ClampedArray(512 * 512 * 4);
    for (let row = 0; row < 512; row++) {
        for (let col = 0; col < 512; col++) {
            const code = (10 * row + col - 100 + 32768) * 256, p = 4 * (row * 512 + col);
            rgba.set([code >> 16, (code >> 8) & 255, code & 255, 255], p);
        }
    }
    return rgba;
}
const height = (col: number, row: number) => 10 * row + col - 100;

describe('climate terrain', () => {
    it('reads the DEM tile that the 512 px terrain source loads for the view', () => {
        expect(demTile(10, 533, 355)).toEqual({ z: 9, x: 266, y: 177, scale: 2, left: 256, top: 256 });
        expect(demTile(0, 0, 0)).toEqual({ z: 0, x: 0, y: 0, scale: 1, left: 0, top: 0 });
        // Above the DEM zoom the deepest DEM tile is cut.
        expect(demTile(15, 8 * 100 + 3, 8 * 50 + 7)).toEqual({ z: 12, x: 100, y: 50, scale: 8, left: 192, top: 448 });
    });

    it('decodes Terrarium heights of the covered part, one DEM pixel per map pixel at the view zoom', () => {
        const rgba = terrarium();
        const quarter = cropHeights(rgba, demTile(10, 533, 355));
        expect([quarter[0], quarter[1], quarter[256], quarter[65_535]]).toEqual([height(256, 256), height(257, 256), height(256, 257), height(511, 511)]);
        const whole = cropHeights(rgba, demTile(0, 0, 0));
        expect([whole[1], whole[256]]).toEqual([height(2, 0), height(0, 2)]);
        const enlarged = cropHeights(rgba, demTile(15, 803, 407));
        expect([enlarged[3], enlarged[4]]).toEqual([height(192, 448), height(193, 448)]);
    });
});
