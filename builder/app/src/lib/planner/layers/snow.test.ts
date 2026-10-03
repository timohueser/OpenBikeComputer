import { describe, expect, it } from 'vitest';
import { dateColumn } from './data-layer';
import { FREE, MOSTLY_FREE, MOSTLY_SNOW, SNOW, UNKNOWN, seasonDay, snowClass, snowGrid, snowStats, type Planar } from './snow';

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

    it('gives the last clear day near the start of the snow season', () => {
        expect(snowStats(route, [0, 1, 3], '2025-10-20', 'light').detail).toBe('Clear until 9 Nov · 20 Oct – 9 Nov');
    });
});

describe('season grid', () => {
    it('shows calendar years, newest last, with seasons split at 1 September', () => {
        const { headline, grid } = snowGrid(planar([[[30, 120]], [[255, 255]]]), 0, 2020, '2021-03-01', 'light');
        expect(headline).toBe('Snow on 1 Mar in 1 of 1 year');
        expect(grid.rows.map(row => row.label)).toEqual(['2021', '2022']);
        expect(grid.marker).toBe(dateColumn('2021-03-01'));
        const [first, last] = grid.rows.map(row => row.cells);
        expect(first[dateColumn('2021-01-01')]).toBe(1);
        expect(first[dateColumn('2021-07-01')]).toBe(0);
        expect(first[dateColumn('2021-09-10')]).toBe(2);
        expect(last[dateColumn('2022-01-01')]).toBe(2);
        expect(last[dateColumn('2022-09-10')]).toBe(255);
    });
});
