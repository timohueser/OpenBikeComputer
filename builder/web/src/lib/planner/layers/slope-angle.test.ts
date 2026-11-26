import { describe, expect, it, vi } from 'vitest';

// map-style reads the page URL when it loads, and map-data loads the release.
vi.stubGlobal('window', { location: { href: 'https://planner.example/plan/' } });
vi.mock('../map-data', () => ({ config: {} }));
const { exactHeights, slopeClasses, slopeLegend, slopeTiles, slopeWords } = await import('./slope-angle');

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

    describe('a map tile', () => {
        // The terrain tile of zoom 12 around Pilatus, and the size of a pixel on the ground in each of its rows.
        const X = 2141, Y = 1440, WORLD = 512 * 4096;
        const pixelMetres = (row: number) => 2 * Math.PI * 6_378_137 / WORLD * Math.cos(Math.atan(Math.sinh(Math.PI * (1 - 2 * (Y * 512 + row + 0.5) / WORLD))));
        /** One terrain tile with a slope to the east, of an angle at each pixel, and no tile beside it; the tiles in `failing` do not load. */
        const terrain = (degrees: (x: number, row: number) => number, failing: string[] = []) => slopeTiles({
            tile: async (_z, x, y) => {
                if (failing.includes(`${x - X},${y - Y}`)) throw new Error('503');
                return x === X && y === Y ? Float32Array.from({ length: 512 * 512 }, (_, i) => (i % 512) * pixelMetres(i >> 9) * tan(degrees(i % 512, i >> 9))) : null;
            },
        });
        const words = async (tiles: ReturnType<typeof slopeTiles>, x: number, y: number) => [...(await slopeWords(tiles, 13, 2 * X + x, 2 * Y + y, new AbortController().signal))!];

        it('measures each row at its own latitude, so ground a little below 30° is clear and a little above is not', async () => {
            // The size of a pixel changes by 0.1 % from one terrain tile to the next: a row of another tile moves these angles across 30°.
            for (const y of [0, 1]) {
                expect(new Set(await words(terrain(() => 29.99), 1, y))).toEqual(new Set([0]));
                expect(new Set(await words(terrain(() => 30.01), 0, y))).toEqual(new Set([0xff00e5f2]));
            }
        });

        it('paints its quarter of the terrain tile', async () => {
            // Steep ground only in the quarter to the south-west.
            const tiles = terrain((x, row) => x < 256 && row >= 256 ? 50 : 0);
            expect(new Set(await words(tiles, 1, 0))).toEqual(new Set([0]));
            expect((await words(tiles, 0, 1)).filter(word => word === 0xffa02882).length).toBeGreaterThan(250 * 250);
        });

        it('paints a tile whose neighbour fails from its own heights, and fails without its own', async () => {
            expect(new Set(await words(terrain(() => 37, ['1,0', '0,-1']), 1, 0))).toEqual(new Set([0xff0096f5]));
            await expect(words(terrain(() => 37, ['0,0']), 0, 0)).rejects.toThrow('503');
        });
    });
});
