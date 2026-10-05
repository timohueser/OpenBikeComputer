import { describe, expect, it } from 'vitest';
import { dateColumn } from './data-layer';
import { COLUMNS, FREE, MOSTLY_FREE, MOSTLY_SNOW, SNOW, UNKNOWN, paintTile, seasonDay, snowChart, snowClass, snowYear, type Planar } from './snow';

/** Planar bytes from per-item [onset, melt] pairs, one list per season. */
function planar(seasons: [number, number][][]): Planar {
    const size = seasons[0].length;
    const data = new Uint8Array(2 * seasons.length * size);
    seasons.forEach((items, s) => items.forEach(([onset, melt], i) => { data[2 * s * size + i] = onset; data[(2 * s + 1) * size + i] = melt; }));
    return { data, size, seasons: seasons.length };
}

describe('snow tiles', () => {
    it('counts days in 2-day steps from 1 September', () => {
        expect(seasonDay('2025-09-01')).toEqual({ season: 2025, index: 0 });
        expect(seasonDay('2026-05-01')).toEqual({ season: 2025, index: 121 });
        expect(seasonDay('2026-08-31')).toEqual({ season: 2025, index: 182 });
        expect(seasonDay('2024-02-29')).toEqual(seasonDay('2024-02-28'));
    });

    it('classes the share of seasons with data that had snow', () => {
        const snow: [number, number] = [10, 50], clear: [number, number] = [60, 90];
        const at = (...seasons: [number, number][]) => snowClass(planar(seasons.map(item => [item])), 0, 30);
        expect(at(clear, [253, 253])).toBe(FREE);
        expect(at(snow, clear, clear, clear)).toBe(MOSTLY_FREE);
        expect(at(snow, clear, clear)).toBe(MOSTLY_SNOW);
        expect(at([254, 254], snow, [253, 253])).toBe(MOSTLY_SNOW);
        expect(at(snow, snow, snow, clear)).toBe(SNOW);
        // A season without data does not count.
        expect(at(snow, [255, 255])).toBe(SNOW);
        expect(at([255, 255], [255, 255])).toBe(UNKNOWN);
    });

    it('draws a missing tile as no data', () => {
        const words = new Uint32Array(256 * 256);
        paintTile(words, undefined, 30, 1, 0, 0, Uint32Array.of(0, 1, 2, 3, 4));
        expect(new Set(words)).toEqual(new Set([0, UNKNOWN]));
        expect([words[0], words[1], words[2], words[256 + 7]]).toEqual([UNKNOWN, UNKNOWN, 0, UNKNOWN]);
    });

    it('blends day values above the data zoom, never with a sentinel', () => {
        // Two pixels across a 4 × 4 part of a tile: melt-out on day 20 and 40, then a no-snow pixel.
        const pixels = new Array(256 * 256).fill([253, 253]) as [number, number][];
        pixels[0] = [0, 20];
        pixels[1] = [0, 40];
        const words = new Uint32Array(256 * 256);
        paintTile(words, planar([pixels]), 30, 64, 0, 0, Uint32Array.of(0, 1, 2, 3, 4));
        // Snow until day 30 reaches halfway between the pixel centres at 32 and 96: a smooth border, not a pixel edge.
        const row = Array.from({ length: 4 }, (_, k) => words[32 * 256 + 32 + 16 * k]);
        expect(row).toEqual([0, 0, SNOW, SNOW]);
        expect(words[32 * 256 + 63]).toBe(0);
        expect(words[32 * 256 + 64]).toBe(SNOW);
        // The no-snow pixel next to them stays snow-free: it gives no days to blend.
        expect(words[32 * 256 + 128 + 32]).toBe(0);
        expect(words[96 * 256 + 32]).toBe(0);
    });
});

describe('route year strip', () => {
    // Three samples; the route melts out on the latest point and snows in on the earliest.
    const route = planar([
        [[30, 120], [25, 140], [253, 253]],
        [[40, 130], [255, 255], [35, 110]],
    ]);

    it('counts the seasons in which the whole route is snow-free', () => {
        const year = snowYear(route, '2026-05-01', 'light');
        expect(year.label).toBe('Whole route snow-free on 1 May: 0 of 2 years');
        expect(year.rows[0].cells[dateColumn('2026-07-01', COLUMNS)]).toBe(FREE);
        expect(year.rows[0].cells[dateColumn('2026-05-01', COLUMNS)]).toBe(SNOW);
    });
});

describe('season grid', () => {
    it('shows one row per calendar year, newest last', () => {
        // Seasons 2020/21 (snow 21 Oct – 1 Mar) and 2021/22 (no data).
        const { headline, grids: [grid] } = snowChart(planar([[[25, 90]], [[255, 255]]]), 0, 2020, '2021-03-01', 'light');
        expect(headline).toBe('Snow on 1 Mar in 1 of 1 year');
        expect(grid.rows.map(row => row.label)).toEqual(['2020', '2021', '2022']);
        const at = (row: number, date: string) => grid.rows[row].cells[dateColumn(date, COLUMNS)];
        // 2020 starts with the archive in September; its winter runs on into the 2021 row.
        expect([at(0, '2020-01-15'), at(0, '2020-09-15'), at(0, '2020-12-15')]).toEqual([2, 0, 1]);
        expect([at(1, '2021-01-15'), at(1, '2021-04-15'), at(1, '2021-10-15')]).toEqual([1, 0, 2]);
        expect([at(2, '2022-01-15'), at(2, '2022-12-15')]).toEqual([2, 2]);
    });
});
