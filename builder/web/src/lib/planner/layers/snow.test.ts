import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../map-types';
import { dateColumn } from './data-layer';
import {
    bandDay, COLUMNS, covering, EVERY, levelClass, paintTile, ROUTE_TILES, routeZoom, RUN_TILE, runImages, seasonDay, seasonsTile, snowBand, snowChart,
    snowClass, snowYear, UNKNOWN, type Planar, type Runs,
} from './snow';

/** Planar bytes from per-item [onset, melt] pairs, one list per season. */
function planar(seasons: [number, number][][]): Planar {
    const size = seasons[0].length;
    const data = new Uint8Array(2 * seasons.length * size);
    seasons.forEach((items, s) => items.forEach(([onset, melt], i) => { data[2 * s * size + i] = onset; data[(2 * s + 1) * size + i] = melt; }));
    return { data, size, seasons: seasons.length };
}

/** The five levels, in table order, from the [first, last] run of each level per pixel. */
const levels = (pixels: [number, number][][]): Runs[] => Array.from({ length: 5 }, (_, level) => ({
    first: Uint8Array.from(pixels, runs => runs[level][0]),
    last: Uint8Array.from(pixels, runs => runs[level][1]),
}));
const same = (run: [number, number]): [number, number][] => Array(5).fill(run);

const PALETTE = Uint32Array.of(0, 1, 2, 3, 4, 5, 6);

describe('snow tiles', () => {
    it('counts days in 2-day steps from 1 September, and bands of 4 steps', () => {
        expect(seasonDay('2025-09-01')).toEqual({ season: 2025, index: 0 });
        expect(seasonDay('2026-05-01')).toEqual({ season: 2025, index: 121 });
        expect(seasonDay('2026-08-31')).toEqual({ season: 2025, index: 182 });
        expect(seasonDay('2024-02-29')).toEqual(seasonDay('2024-02-28'));
        // 26 February and 1 March are in band 22, whose day is 1 March.
        expect([snowBand(seasonDay('2026-02-26').index), snowBand(seasonDay('2026-03-01').index), bandDay(22)]).toEqual([22, 22, 90]);
        expect([snowBand(182), bandDay(45)]).toEqual([45, 182]);
    });

    it('classes a sample by the seasons with snow on the band day', () => {
        const snow: [number, number] = [10, 50], clear: [number, number] = [60, 90];
        // Band 7 counts day 30.
        const at = (...seasons: [number, number][]) => snowClass(planar(seasons.map(item => [item])), 0, 7);
        expect(at(clear, [253, 253])).toBe(0);
        expect(at(snow, clear, clear, clear, clear), 'below 1/4').toBe(1);
        expect(at(snow, clear, clear, clear), '1/4').toBe(2);
        expect(at(snow, snow, clear, clear), '1/2').toBe(3);
        expect(at([254, 254], snow, snow, clear), '3/4').toBe(4);
        expect(at(snow, snow, snow)).toBe(EVERY);
        // A season without data does not count.
        expect(at(snow, [255, 255])).toBe(EVERY);
        expect(at([255, 255], [255, 255])).toBe(UNKNOWN);
        // Snow until 26 February is clear on its band day, 1 March.
        expect(snowClass(planar([[[0, seasonDay('2026-02-26').index]]]), 0, snowBand(seasonDay('2026-02-26').index))).toBe(0);
    });

    it('classes a pixel by the levels that hold the band from any up, also when the runs do not nest', () => {
        const runs = levels([
            [[5, 30], [8, 25], [10, 20], [12, 18], [14, 16]],
            // Two snow periods: the any level keeps the later run, the quarter level the earlier one.
            [[10, 30], [2, 6], [12, 14], [253, 253], [253, 253]],
            same([253, 253]),
            same([255, 255]),
        ]);
        expect([3, 6, 9, 11, 13, 15, 31].map(band => levelClass(runs, 0, band))).toEqual([0, 1, 2, 3, 4, 5, 0]);
        expect(levelClass(runs, 1, 13), 'the half level does not count above a gap').toBe(1);
        expect(levelClass(runs, 1, 4), 'the quarter level does not count without any').toBe(0);
        expect([levelClass(runs, 2, 10), levelClass(runs, 3, 10)]).toEqual([0, UNKNOWN]);
    });

    it('reads the nearest ancestor of a missing run tile down to zoom 7', async () => {
        const held = new Set(['7/1/1', '9/4/4', '10/8/8']);
        const read = (z: number, x: number, y: number) => Promise.resolve(held.has(`${z}/${x}/${y}`) ? `${z}/${x}/${y}` : null);
        expect(await covering(read, 11, 11, 17, 17)).toEqual({ data: '10/8/8', zoom: 10 });
        expect(await covering(read, 11, 13, 75, 75), 'above the max zoom, then up').toEqual({ data: '9/4/4', zoom: 9 });
        expect(await covering(read, 11, 11, 20, 20)).toEqual({ data: '7/1/1', zoom: 7 });
        expect(await covering(read, 11, 8, 4, 4), 'no ancestor below zoom 7').toEqual({ data: null, zoom: 7 });
    });

    it('splits a run tile into its images by their RIFF sizes', () => {
        const image = (payload: number) => {
            const bytes = new Uint8Array(8 + payload);
            new DataView(bytes.buffer).setUint32(4, payload, true);
            return [...bytes];
        };
        expect(runImages(Uint8Array.from([...image(6), ...image(4)])).map(part => part.length)).toEqual([14, 12]);
        expect(() => runImages(Uint8Array.from([...image(6), 0, 0]))).toThrow();
    });

    it('draws a missing tile as no data', () => {
        const words = new Uint32Array(256 * 256);
        paintTile(words, undefined, 7, 2, 0, 0, PALETTE);
        expect(new Set(words)).toEqual(new Set([0, UNKNOWN]));
        expect([words[0], words[1], words[2], words[256 + 7]]).toEqual([UNKNOWN, UNKNOWN, 0, UNKNOWN]);
    });

    it('draws a class border above the data zoom where a level has half of the bilinear weight', () => {
        // Data pixels (column, row): snow every year on (0, 0) and (1, 0), class 3 on (0, 1), none on
        // the others of columns 0 to 2, and no data from column 3.
        const plane = () => new Uint8Array(RUN_TILE * RUN_TILE).fill(253);
        const runs: Runs[] = Array.from({ length: 5 }, () => ({ first: plane(), last: plane() }));
        const put = (i: number, [first, last]: [number, number], count = 5) => runs.slice(0, count).forEach(level => { level.first[i] = first; level.last[i] = last; });
        put(0, [0, 45]); put(1, [0, 45]); put(RUN_TILE, [0, 45], 3);
        for (let i = 0; i < RUN_TILE * RUN_TILE; i++) if (i % RUN_TILE >= 3) put(i, [255, 255]);
        const words = new Uint32Array(256 * 256);
        paintTile(words, runs, 10, 2, 0, 0, PALETTE);
        expect([words[0], words[1], words[256], words[257], words[2], words[8]], 'one screen pixel for each data pixel').toEqual([EVERY, EVERY, 3, 0, 0, UNKNOWN]);
        // 64 parts: 32 screen pixels for each data pixel, with the centres at 16, 48, 80 and 112.
        words.fill(0);
        paintTile(words, runs, 10, 64, 0, 0, PALETTE);
        const at = (px: number, py: number) => words[py * 256 + px];
        // A straight border stays on the pixel edge.
        expect([at(16, 31), at(16, 32), at(63, 16), at(64, 16)]).toEqual([EVERY, 3, EVERY, 0]);
        // The inner corner of the three snow pixels fills past the pixel edges at 32, and each level has its own border.
        expect([at(36, 36), at(40, 40)]).toEqual([3, 0]);
        // The outer corner of (1, 0) is cut.
        expect([at(52, 28), at(56, 28)]).toEqual([EVERY, 0]);
        // The hatch starts where no data has more than half of the weight, on its lines.
        expect([at(95, 17), at(96, 16), at(100, 20)]).toEqual([0, UNKNOWN, UNKNOWN]);
    });

    it('reads the zoom 10 ancestor of a missing seasons tile', async () => {
        const held = new Set(['10/536/361', '12/2144/1444']);
        const read = (z: number, x: number, y: number) => Promise.resolve(held.has(`${z}/${x}/${y}`) ? `${z}/${x}/${y}` : null);
        expect(await seasonsTile(read, 12, 2144, 1444)).toEqual({ data: '12/2144/1444', zoom: 12 });
        expect(await seasonsTile(read, 14, 8580, 5780)).toEqual({ data: '10/536/361', zoom: 10 });
        expect(await seasonsTile(read, 10, 537, 361)).toEqual({ data: null, zoom: 10 });
    });

    it('samples a route at the finest zoom with at most 25 tiles', () => {
        // One point in each of a row of zoom 14 tiles.
        const line = (tiles: number): Coordinate[] => Array.from({ length: tiles }, (_, k) => [(8576 + k + 0.5) / 2 ** 14 * 360 - 180, 47]);
        expect(routeZoom(line(1), 2, 14)).toBe(14);
        expect(routeZoom(line(ROUTE_TILES), 2, 14)).toBe(14);
        expect(routeZoom(line(ROUTE_TILES + 1), 2, 14), 'one zoom coarser').toBe(13);
        expect(routeZoom(line(4 * ROUTE_TILES), 2, 14)).toBe(12);
        expect(routeZoom(line(4 * ROUTE_TILES), 13, 14), 'the min zoom where none fits').toBe(13);
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
        expect(year.fills).toHaveLength(7);
        const cell = (date: string) => year.rows[0].cells[dateColumn(date, COLUMNS)];
        expect([cell('2026-07-01'), cell('2026-05-28'), cell('2026-05-01')]).toEqual([0, 3, EVERY]);
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
