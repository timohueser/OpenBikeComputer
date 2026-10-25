import { describe, expect, it } from 'vitest';
import { dateColour, groundPages, snowState, type Raster } from './ground-pages';

describe('the ground pages', () => {
    it('blends the two quarters about the date, and a quarter that saw nothing hands its weight to the other', () => {
        const one = (pixel: number[]): Raster => ({ z: 0, x: 0, y: 0, size: 1, channels: 4, data: Uint8Array.from(pixel) });
        const both = dateColour(one([100, 100, 100, 200]), one([200, 200, 200, 128]), 0.25);
        expect(both.rgb[0][0]).toBeCloseTo((0.75 * 100 + 0.25 * 200) / 255, 5);
        expect(both.ndvi[0]).toBeCloseTo(0.75 * (199 / 127 - 1), 5);
        const alone = dateColour(one([100, 100, 100, 200]), one([0, 0, 0, 0]), 0.25);
        expect([alone.rgb[0][0], alone.seen[0]]).toEqual([Math.fround(100 / 255), 1]);
        // A quarter that saw snow measured no NDVI, and the nearer quarter's mark goes with the colour.
        const snow = dateColour(one([100, 0, 0, 1]), one([0, 200, 0, 150]), 0.25);
        expect(snow.mark[0]).toBe(1);
        expect(snow.ndvi[0]).toBeCloseTo(0.25 * (149 / 127 - 1), 5);
    });

    it('has snow where at least the share of seasons with data had snow on the day', () => {
        // Seasons with data × 16 + seasons with snow.
        const day = (byte: number): Raster => ({ z: 0, x: 0, y: 0, size: 1, channels: 1, data: Uint8Array.from([byte]) });
        expect(snowState(day(2 * 16 + 1), () => 0, [0.5]).map(values => values[0])).toEqual([1, 1, 0.5]);
        expect(snowState(day(2 * 16 + 1), () => 0, [0.6]).map(values => values[0])).toEqual([0, 1, 0.5]);
        expect(snowState(day(0), () => 0, [0]).map(values => values[0])).toEqual([0, 0, 0]);
    });

    it('splits a ground tile into its four pages, and a region that the rules do not have is the world', () => {
        const ground: Raster = { z: 3, x: 5, y: 2, size: 512, channels: 4, data: new Uint8Array(4 * 512 * 512) };
        // The west half has ground: material 13, barren share 77, lithology 8, and region 1 in the west quarter, 9 east of it.
        for (let i = 0; i < 512 * 512; i++) ground.data.set(i % 512 < 256 ? [13, 77, 8, i % 512 < 128 ? 1 : 9] : [0x80, 0, 0, 255], 4 * i);
        const pages = groundPages({ z: 3, x: 5, y: 2, ground, date: { weight: 0, snowIndex: 0, shares: [0.5, 0.5] }, world: 0 });
        expect(pages.map(Boolean)).toEqual([true, false, true, false]);
        const [nw] = pages;
        expect(Array.from(nw!.cells.subarray(0, 4))).toEqual([13, 8, 1, 77]);
        expect(nw!.cells[4 * 200 + 2]).toBe(0);
    });
});
