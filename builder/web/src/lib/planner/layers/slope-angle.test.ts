import { describe, expect, it, vi } from 'vitest';

// map-style reads the page URL when it loads, and map-data loads the release.
vi.stubGlobal('window', { location: { href: 'https://planner.example/plan/' } });
vi.mock('../map-data', () => ({ config: {} }));
const { exactHeights, paintSlope, slopeClasses, slopeLegend } = await import('./slope-angle');

const SIZE = 4, PIXEL_M = 13;
const tan = (degrees: number) => Math.tan(degrees * Math.PI / 180);
/** A tile with a height at each (column, row). */
const tile = (height: (x: number, y: number) => number) => Float32Array.from({ length: SIZE * SIZE }, (_, i) => height(i % SIZE, Math.floor(i / SIZE)));
const classes = (heights: Float32Array, beside = {}, pixelMetres = (_row: number) => PIXEL_M) => [...slopeClasses(heights, beside, pixelMetres, SIZE)];
const all = (value: number) => Array(SIZE * SIZE).fill(value);

describe('slope angle', () => {
    it('gives a plane the class of its angle, in each direction, also at the edges of a tile with no tile beside it', () => {
        const east = (degrees: number) => classes(tile(x => x * PIXEL_M * tan(degrees)));
        expect([29, 31, 37, 42, 50].map(degrees => east(degrees))).toEqual([0, 1, 2, 3, 4].map(all));
        expect(classes(tile((_x, y) => -y * PIXEL_M * tan(37)))).toEqual(all(2));
        // The steepest line of this plane runs to the south-east.
        expect(classes(tile((x, y) => (x + y) * PIXEL_M * tan(42) / Math.SQRT2))).toEqual(all(3));
        expect(slopeLegend).toEqual({ swatches: [expect.objectContaining({ label: '30–35°' }), expect.objectContaining({ label: '35–40°' }),
            expect.objectContaining({ label: '40–45°' }), expect.objectContaining({ label: '> 45°' })] });
    });

    it('leaves flat ground clear', () => {
        expect(classes(tile(() => 1200.4))).toEqual(all(0));
    });

    it('measures each row with the size of its pixels on the ground', () => {
        // The same heights are steeper where a pixel is smaller.
        const heights = tile(x => x * PIXEL_M * tan(29));
        expect(classes(heights, {}, row => row < 2 ? PIXEL_M : PIXEL_M / 2)).toEqual([...Array(8).fill(0), ...Array(8).fill(4)]);
    });

    it('leaves a pixel with no known height clear, and its neighbours read the slope from their own side', () => {
        const heights = tile(x => x * PIXEL_M * tan(37));
        heights[0] = NaN;
        expect(classes(heights)).toEqual([0, ...all(2).slice(1)]);
    });

    it('reads the neighbour of an edge pixel from the line of the next tile that touches it', () => {
        const flat = tile(() => 0);
        /** The four tiles beside, each with one height in the line that touches the tile and another in its other lines. */
        const beside = (touching: number, other: number) => ({
            north: tile((_x, y) => y === SIZE - 1 ? touching : other), south: tile((_x, y) => y === 0 ? touching : other),
            west: tile(x => x === SIZE - 1 ? touching : other), east: tile(x => x === 0 ? touching : other),
        });
        expect(classes(flat, beside(0, 100))).toEqual(all(0));
        expect(classes(flat, beside(100, 0))).toEqual([4, 4, 4, 4, 4, 0, 0, 4, 4, 0, 0, 4, 4, 4, 4, 4]);
        expect(classes(flat, { east: beside(100, 0).east })).toEqual([0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 4, 0, 0, 0, 4]);
    });

    it('decodes a height with its fraction of a metre', () => {
        const [low, high, unknown] = exactHeights([128, 0, 128, 255, 134, 7, 51, 255, 134, 7, 51, 0], 3);
        expect([low, high, unknown]).toEqual([0.5, 1543 + 51 / 256, NaN]);
    });

    it('paints the part of the terrain tile that a map tile covers, one pixel for one pixel', () => {
        const terrain = new Uint8Array(512 * 512), words = new Uint32Array(256 * 256);
        terrain[5 * 512 + 256 + 3] = 4;
        paintSlope(words, terrain, { left: 256, top: 0 });
        expect([...words].flatMap((word, i) => word ? [[i, word]] : [])).toEqual([[5 * 256 + 3, 0xffa02882]]);
    });
});
