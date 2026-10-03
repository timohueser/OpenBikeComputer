import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../map-types';
import {
    DETAIL, OVERVIEW, RAIN_DRIER, RAIN_TYPICAL, RAIN_UNKNOWN, RAIN_WETTER, SECTORS, WETTER_RATIO, cellAt, cellHistory, climateMeta,
    climateTile, locate, rainClass, rainRatios, read, temperatureAt, weekMonth, weekOf, weekValues, wetDaysOf7, windChance, windMode, windRose,
    type Level,
} from './climate';
import { lineBearings, weatherRows, windRow } from './climate-route';
import { sampleLine, type TileGetter } from './climate-source';

// The plane tables of specs/planner-climate-tiles.md: name, indexes, bytes per value.
const SPEC: Record<Level, { cells: number; planes: [string, number, number][] }> = {
    [OVERVIEW]: { cells: 384, planes: [['orography', 1, 2], ['lapse_tmax', 12, 1], ['lapse_tmin', 12, 1], ['rose', 192, 1], ['wet_share', 52, 1],
        ['rain', 52, 1], ['tmax', 52, 1], ['tmin', 52, 1], ['wind', 52, 1]] },
    [DETAIL]: { cells: 96, planes: [['orography', 1, 2], ['lapse_tmax', 12, 1], ['lapse_tmin', 12, 1], ['wet_days', 520, 1], ['rain', 520, 1],
        ['tmax', 520, 1], ['tmin', 520, 1], ['wind', 520, 1]] },
};
const SIGNED = new Set(['orography', 'lapse_tmax', 'lapse_tmin', 'tmax', 'tmin']);

/** A tile body with every value missing, and a writer of raw codes. */
function specTile(level: Level, x = 0, y = 0) {
    const { cells, planes } = SPEC[level];
    const starts = new Map<string, number>();
    let size = 0;
    for (const [name, count, bytes] of planes) { starts.set(name, size); size += count * cells * bytes; }
    const body = new Uint8Array(size);
    const view = new DataView(body.buffer);
    const set = (name: string, index: number, cell: number, code: number) => {
        const [, , bytes] = planes.find(p => p[0] === name)!;
        const at = starts.get(name)! + (index * cells + cell) * bytes;
        if (bytes === 2) view.setInt16(at, code, true);
        else if (SIGNED.has(name)) view.setInt8(at, code);
        else view.setUint8(at, code);
    };
    for (const [name, count] of planes) for (let i = 0; i < count; i++) for (let c = 0; c < cells; c++) set(name, i, c, name === 'orography' ? -32768 : SIGNED.has(name) ? -128 : 255);
    return { body, set, tile: () => climateTile(level, x, y, body) };
}

describe('climate tiles', () => {
    it('has the body sizes of the spec', () => {
        expect(specTile(OVERVIEW).body.length).toBe(183_552);
        expect(specTile(DETAIL).body.length).toBe(252_096);
        expect(() => climateTile(DETAIL, 0, 0, new Uint8Array(183_552))).toThrow('183552 bytes');
    });

    it('decodes each code type and reads missing as NaN', () => {
        const t = specTile(OVERVIEW);
        t.set('orography', 0, 5, 1234);
        t.set('orography', 0, 6, -12);
        t.set('lapse_tmax', 3, 5, -65);
        t.set('rose', 191, 383, 200);
        t.set('rain', 0, 5, 100);
        t.set('rain', 1, 5, 101);
        t.set('rain', 2, 5, 254);
        t.set('tmax', 10, 5, -3);
        t.set('wind', 51, 383, 7);
        const tile = t.tile();
        expect([read(tile, 'orography', 0, 5), read(tile, 'orography', 0, 6), read(tile, 'orography', 0, 7)]).toEqual([1234, -12, NaN]);
        expect(read(tile, 'lapse_tmax', 3, 5)).toBeCloseTo(-6.5);
        expect(read(tile, 'rose', 191, 383)).toBe(100);
        expect([0, 1, 2].map(week => read(tile, 'rain', week, 5))).toEqual([100, 105, 870]);
        expect([read(tile, 'tmax', 10, 5), read(tile, 'tmax', 10, 6)]).toEqual([-1.5, NaN]);
        // The last value of the body.
        expect(read(tile, 'wind', 51, 383)).toBe(3.5);
        expect(weekValues(tile, 'rain', 1)[5]).toBe(105);
        expect(() => read(tile, 'wet_days', 0, 0)).toThrow('no wet_days');
    });

    it('reads every year and week of a detail cell', () => {
        const t = specTile(DETAIL);
        t.set('wet_days', 52 * 9 + 51, 95, 9);
        t.set('tmin', 52 * 3 + 7, 95, -20);
        const history = cellHistory(t.tile(), 95);
        expect(history.wetDays[519]).toBe(9);
        expect(history.tmin[52 * 3 + 7]).toBe(-10);
        expect(Number.isNaN(history.rain[0])).toBe(true);
    });

    it('validates the metadata', () => {
        expect(climateMeta({ first_year: 2016, years: 10, wet_day_mm: 2.3, attribution: 'ERA5-Land' })).toEqual({ firstYear: 2016, years: 10, wetDayMm: 2.3, attribution: 'ERA5-Land' });
        expect(() => climateMeta({ first_year: 2016, years: 9, wet_day_mm: 2.3 })).toThrow();
    });

    it('finds the cell and tile of a coordinate on both levels', () => {
        const freiburg = cellAt([7.86, 47.99]);
        expect(freiburg).toEqual({ col: 1879, row: 420 });
        expect(locate(OVERVIEW, freiburg)).toEqual({ x: 78, y: 26, index: 4 * 24 + 7 });
        expect(locate(DETAIL, freiburg)).toEqual({ x: 156, y: 52, index: 4 * 12 + 7 });
        // The column wraps at the antimeridian.
        expect(cellAt([179.96, 90])).toEqual({ col: 0, row: 0 });
        expect(cellAt([-180, 0]).col).toBe(0);
    });

    it('counts weeks from 1 January, with the last days in week 51', () => {
        expect(['2026-01-01', '2026-01-07', '2026-01-08', '2026-12-30', '2024-12-31'].map(weekOf)).toEqual([0, 0, 1, 51, 51]);
        expect([weekMonth(0), weekMonth(4), weekMonth(51)]).toEqual([0, 1, 11]);
    });
});

describe('climate values', () => {
    it('moves temperatures to an elevation with the lapse rate of the month', () => {
        const t = specTile(DETAIL);
        const week = 52 * 2 + 4; // February
        t.set('orography', 0, 0, 500);
        t.set('tmax', week, 0, 20);
        t.set('tmin', week, 0, 0);
        t.set('lapse_tmax', 1, 0, -65);
        t.set('lapse_tmin', 1, 0, -20);
        t.set('lapse_tmin', 0, 0, -127);
        const tile = t.tile();
        expect(temperatureAt(tile, 'tmax', week, 0, 1500)).toBeCloseTo(3.5);
        expect(temperatureAt(tile, 'tmin', week, 0, 0)).toBeCloseTo(1);
        expect(temperatureAt(tile, 'tmax', week, 0)).toBe(10);
        expect(temperatureAt(tile, 'tmax', week, 0, NaN)).toBe(10);
    });

    it('gives wet days as days of 7', () => {
        const t = specTile(OVERVIEW);
        t.set('wet_share', 20, 0, 34);
        expect(wetDaysOf7(t.tile(), 0, 20)).toBeCloseTo(2.38);
    });

    it('measures rain against the mean week of the same series', () => {
        // 1 mm a day. 2016, 2020 and 2024 are leap years: week 51 has 8.3 days in the overview mean.
        const even = Array.from({ length: 52 }, (_, week) => week === 51 ? 8.3 : 7);
        expect([...rainRatios(even, 2016)].every(ratio => Math.abs(ratio - 1) < 1e-6)).toBe(true);
        const ratios = rainRatios(even.map((mm, week) => week === 10 ? 2 * mm : mm), 2016);
        expect(ratios[10] / ratios[11]).toBeCloseTo(2);
        // Detail week 51 has 9 days in the leap years 2016, 2020 and 2024, else 8; slot 52 is week 0 of 2017.
        const detail = Array.from({ length: 520 }, (_, i): number => i % 52 !== 51 ? 7 : [0, 4, 8].includes(Math.floor(i / 52)) ? 9 : 8);
        expect([...rainRatios(detail, 2016)].every(ratio => Math.abs(ratio - 1) < 1e-6)).toBe(true);
        detail[52] *= 3;
        detail[60] = NaN;
        const years = rainRatios(detail, 2016);
        expect(years[52] / years[0]).toBeCloseTo(3);
        expect(Number.isNaN(years[60])).toBe(true);
        expect(Number.isNaN(rainRatios([0, 0], 2016)[0])).toBe(true);
        expect([1 / WETTER_RATIO - 0.01, 1, WETTER_RATIO + 0.01, NaN].map(rainClass)).toEqual([RAIN_DRIER, RAIN_TYPICAL, RAIN_WETTER, RAIN_UNKNOWN]);
    });

    it('turns the wind rose to the direction the wind blows towards', () => {
        const t = specTile(OVERVIEW);
        for (let s = 0; s < SECTORS; s++) t.set('rose', 2 * SECTORS + s, 0, s === 4 ? 150 : 10); // March, mostly from the east
        const rose = windRose(t.tile(), 0, 2);
        expect(rose.reduce((a, b) => a + b)).toBeCloseTo(1);
        expect(rose.indexOf(Math.max(...rose))).toBe(12);
        expect(windMode(rose)?.towards).toBe(12);
        expect(windMode(windRose(t.tile(), 1, 2))).toBeUndefined();
    });

    it('finds the main direction, its steadiness and a second opposite direction', () => {
        const rose = (shares: Record<number, number>) => {
            const rest = (1 - Object.values(shares).reduce((a, b) => a + b, 0)) / (SECTORS - Object.keys(shares).length);
            return Array.from({ length: SECTORS }, (_, s) => shares[s] ?? rest);
        };
        expect(windMode(rose({ 3: 1 }))).toEqual({ towards: 3, steadiness: 1 });
        expect(windMode(rose({}))?.steadiness).toBeCloseTo(3 / 16);
        expect(windMode(rose({}))?.opposite).toBeUndefined();
        // Valley channelling: up and down the valley, little across it.
        expect(windMode(rose({ 15: 0.1, 0: 0.25, 1: 0.1, 7: 0.1, 8: 0.2, 9: 0.1 }))).toMatchObject({ towards: 0, opposite: 8 });
        // A second direction up to 45° off straight opposite still counts.
        expect(windMode(rose({ 0: 0.4, 9: 0.05, 10: 0.2, 11: 0.05 }))?.opposite).toBe(10);
        // Westerlies with a weak return are one direction.
        const weak = windMode(rose({ 3: 0.15, 4: 0.25, 5: 0.15, 12: 0.12 }));
        expect(weak?.towards).toBe(4);
        expect(weak?.opposite).toBeUndefined();
    });

    it('splits the wind into head, cross and tail chances for a bearing', () => {
        const north = Array.from({ length: SECTORS }, (_, s) => s === 0 ? 1 : 0); // blows towards the north
        expect(windChance(north, 0)).toEqual({ head: 0, cross: 0, tail: 1 });
        expect(windChance(north, 180)).toEqual({ head: 1, cross: 0, tail: 0 });
        expect(windChance(north, 90)).toEqual({ head: 0, cross: 1, tail: 0 });
        expect(windChance(north, 225)).toEqual({ head: 0.5, cross: 0.5, tail: 0 });
        expect(windChance(north, 213.75).head).toBe(1);
        const even = Array.from({ length: SECTORS }, () => 1 / SECTORS);
        for (const bearing of [0, 10, 100, 359]) {
            const chance = windChance(even, bearing);
            expect(chance.head).toBeCloseTo(0.25);
            expect(chance.tail).toBeCloseTo(0.25);
        }
    });
});

/** A tile getter over fixed tiles that counts its requests. */
function counted(tiles: ReturnType<typeof climateTile>[]) {
    const requests: string[] = [];
    const get: TileGetter = async (level, x, y) => {
        requests.push(`${level}/${x}/${y}`);
        return tiles.find(t => t.level === level && t.x === x && t.y === y);
    };
    return { requests, get };
}

describe('climate along a route', () => {
    const FREIBURG = 4 * 24 + 7, EAST = FREIBURG + 1;
    function overview() {
        const t = specTile(OVERVIEW, 78, 26);
        for (const [cell, high] of [[FREIBURG, 20], [EAST, 40]]) {
            t.set('orography', 0, cell, 0);
            for (let week = 0; week < 52; week++) { t.set('tmax', week, cell, high); t.set('tmin', week, cell, 0); t.set('wet_share', week, cell, week); }
            for (let month = 0; month < 12; month++) {
                t.set('lapse_tmax', month, cell, -65);
                t.set('lapse_tmin', month, cell, -65);
                t.set('rose', 16 * month + 8, cell, 200); // from the south: blows towards the north
                for (let s = 0; s < 16; s++) if (s !== 8) t.set('rose', 16 * month + s, cell, 0);
            }
        }
        return t.tile();
    }

    it('requests each tile of a line once', async () => {
        const { requests, get } = counted([overview()]);
        const line: Coordinate[] = Array.from({ length: 50 }, (_, i) => [7.86 + i * 0.002, 47.99]);
        const cells = await sampleLine([...line, [9.6, 47.99]], get);
        expect(requests).toEqual(['8/78/26', '8/79/26']);
        expect(cells[0]?.index).toBe(FREIBURG);
        expect(cells[49]?.index).toBe(EAST);
        expect(cells[50]).toBeUndefined();
    });

    it('averages the weather rows by distance, at the elevation of each sample', async () => {
        const { get } = counted([overview()]);
        const cells = await sampleLine([[7.86, 47.99], [7.96, 47.99]], get);
        const rows = weatherRows(cells, [0, 10]);
        expect(rows.high.length).toBe(52);
        expect(rows.high[0]).toBe(15);
        expect(rows.rain[14]).toBeCloseTo(0.98);
        expect(weatherRows(cells, [0, 10], [1000, 0]).high[0]).toBeCloseTo(11.75);
        // Samples in one cell: the mean of their own corrections.
        const valley = await sampleLine([[7.86, 47.99], [7.87, 47.99], [7.88, 47.99]], get);
        expect(weatherRows(valley, [0, 1, 2], [0, 0, 2000]).high[0]).toBeCloseTo((0.5 * 10 + 1 * 10 + 0.5 * -3) / 2);
        expect(Number.isNaN(weatherRows([undefined], [0]).low[0])).toBe(true);
    });

    it('gives the headwind chance per month for the travel direction', async () => {
        const { get } = counted([overview()]);
        const north: Coordinate[] = [[7.86, 47.99], [7.96, 48.0]];
        const cells = await sampleLine(north, get);
        expect([...windRow(cells, [0, 8], lineBearings(north))]).toEqual(new Array(12).fill(0));
        expect([...windRow(cells, [0, 8], [180, 180])]).toEqual(new Array(12).fill(1));
    });

    it('measures the travel bearing between the neighbours of each point', () => {
        expect([...lineBearings([[0, 0], [1, 0], [1, 1]])].map(Math.round)).toEqual([90, 45, 0]);
        expect(Math.round(lineBearings([[10, 60], [9, 60]])[0])).toBe(270);
    });
});
