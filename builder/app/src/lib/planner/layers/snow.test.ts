import { describe, expect, it } from 'vitest';
import { dateColumn, nearestStored } from './data-layer';
import { FREE, MOSTLY_FREE, MOSTLY_SNOW, SNOW, UNKNOWN, indexLabel, paintTile, seasonDay, snowClass, snowGrid, snowStats, type Planar } from './snow';

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

    it('takes the nearest stored ancestor of an omitted tile', async () => {
        const stored = new Map([['10/1/2', { data: 'a', z: 10 }], ['9/0/1', { data: 'b', z: 9 }]]);
        const asked: string[] = [];
        const tile = async (z: number, x: number, y: number) => { asked.push(`${z}/${x}/${y}`); return stored.get(`${z}/${x}/${y}`); };
        expect(await nearestStored(tile, 12, 6, 9, 0)).toEqual({ data: 'a', z: 10 });
        expect(asked).toEqual(['12/6/9', '11/3/4', '10/1/2']);
        expect(await nearestStored(tile, 12, 0, 8, 0)).toEqual({ data: 'b', z: 9 });
        expect(await nearestStored(tile, 12, 0, 0, 8)).toBeUndefined();
        // A tile service answers with the ancestor itself, so one request is enough.
        asked.length = 0;
        expect(await nearestStored(async (z, x, y) => (asked.push(`${z}/${x}/${y}`), { data: 'c', z: 9 }), 12, 6, 9, 0)).toEqual({ data: 'c', z: 9 });
        expect(asked).toEqual(['12/6/9']);
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

    it('ends the last day index on 31 August', () => {
        expect(indexLabel(182)).toBe('31 Aug');
        expect(indexLabel(182, true)).toBe('31 Aug');
        expect(indexLabel(0, true)).toBe('2 Sept');
    });
});

describe('route stats', () => {
    // Three samples over 3 km; the route melts out on the latest point and snows in on the earliest.
    const route = planar([
        [[30, 120], [25, 140], [253, 253]],
        [[40, 130], [255, 255], [35, 110]],
    ]);

    it('gives snowed-in km and the median first clear day in spring', () => {
        const stats = snowStats(route, [0, 1, 3], '2026-05-01', 'light');
        expect(stats.headline).toBe('1.5 km snowed in on 1 May');
        expect(stats.detail).toBe('Clear from 10 Jun · 21 May – 10 Jun');
        expect(stats.year.label).toBe('Whole route snow-free on 1 May: 0 of 2 years');
        expect(stats.year.grid.rows[0].cells[dateColumn('2026-07-01')]).toBe(FREE);
    });

    it('gives the next change after the date', () => {
        const detail = (date: string) => snowStats(route, [0, 1, 3], date, 'light').detail;
        expect(detail('2026-01-21')).toBe('Clear from 10 Jun · 21 May – 10 Jun');
        expect(detail('2026-07-15')).toBe('Clear until 9 Nov · 20 Oct – 9 Nov');
        expect(detail('2025-10-20')).toBe('Clear until 9 Nov · 20 Oct – 9 Nov');
    });

    it('gives no dates for seasons without snow', () => {
        // Snow from 30 Oct to 20 Mar in one of three seasons.
        const sometimes = planar([[[30, 100]], [[253, 253]], [[253, 253]]]);
        const detail = (date: string) => snowStats(sometimes, [0], date, 'light').detail;
        expect(detail('2026-01-10')).toBe('Usually clear on this date');
        expect(detail('2026-05-01')).toBe('Snow in only 1 of 3 years');
        const mostly = planar([[[30, 121]], [[30, 140]], [[253, 253]]]);
        expect(snowStats(mostly, [0], '2026-01-10', 'light').detail).toBe('Clear from 10 Jun · 3 May – 10 Jun');
    });
});

describe('season grid', () => {
    it('shows one row per season from September to August, newest last', () => {
        const { headline, grid } = snowGrid(planar([[[30, 120]], [[255, 255]]]), 0, 2020, '2021-03-01', 'light');
        expect(headline).toBe('Snow on 1 Mar in 1 of 1 year');
        expect(grid.seasonal).toBe(true);
        expect(grid.rows.map(row => row.label)).toEqual(['2020/21', '2021/22']);
        const column = (date: string) => seasonDay(date).index;
        expect(grid.marker).toBe(column('2021-03-01'));
        const [first, last] = grid.rows.map(row => row.cells);
        expect([first[0], first[column('2020-10-20')], first[column('2020-11-10')], first[column('2021-01-01')], first[column('2021-07-01')]]).toEqual([0, 0, 1, 1, 0]);
        expect([last[0], last[182]]).toEqual([2, 2]);
    });
});
