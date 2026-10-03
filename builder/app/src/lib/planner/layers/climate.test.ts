import { existsSync, readFileSync } from 'node:fs';
import { PMTiles } from 'pmtiles';
import { describe, expect, it } from 'vitest';
import type { Coordinate } from '../map-types';
import {
    DETAIL, OVERVIEW, RAIN_DRIER, RAIN_TYPICAL, RAIN_UNKNOWN, RAIN_WETTER, SECTORS, WETTER_RATIO, cellAt, cellHistory, climateMeta,
    climateTile, locate, rainClass, rainRatios, read, temperatureAt, weekMonth, weekOf, weekValues, wetDaysOf7, windChance, windMode, windRose,
    type Level,
} from './climate';
import { lineBearings, weatherRows, windRow } from './climate-route';
import { climateSource, sampleLine, type TileGetter } from './climate-source';
import type { TileArchive } from './tile-archive';

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
        const even = Array.from({ length: 52 }, (_, week) => week === 51 ? 8.25 : 7);
        expect([...rainRatios(even)].every(ratio => Math.abs(ratio - 1) < 1e-6)).toBe(true);
        const ratios = rainRatios(even.map((mm, week) => week === 10 ? 2 * mm : mm));
        expect(ratios[10] / ratios[11]).toBeCloseTo(2);
        // A detail series skips missing weeks; slot 52 is week 0 of year 1.
        const detail = [...even, ...even.map(mm => mm * 3)];
        detail[60] = NaN;
        const years = rainRatios(detail);
        expect(years[52] / years[0]).toBeCloseTo(3);
        expect(Number.isNaN(years[60])).toBe(true);
        expect(Number.isNaN(rainRatios([0, 0])[0])).toBe(true);
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

const ARCHIVE = '/private/tmp/claude-501/-Users-timo-Documents-OSM/3eed0a5d-1a70-4f42-846d-9b73388587bb/scratchpad/climate/climate.pmtiles';

/** Points every 100 m along the waypoints, with the distance of each in km. */
function densify(waypoints: Coordinate[]): { line: Coordinate[]; km: number[] } {
    const line: Coordinate[] = [], km: number[] = [];
    let total = 0;
    for (let i = 1; i < waypoints.length; i++) {
        const [[lon0, lat0], [lon1, lat1]] = [waypoints[i - 1], waypoints[i]];
        const length = 111.2 * Math.hypot((lon1 - lon0) * Math.cos((lat0 + lat1) * Math.PI / 360), lat1 - lat0);
        for (let step = 0; step < length / 0.1; step++) {
            const f = step * 0.1 / length;
            line.push([lon0 + f * (lon1 - lon0), lat0 + f * (lat1 - lat0)]);
            km.push(total + step * 0.1);
        }
        total += length;
    }
    return { line, km };
}

describe.skipIf(!existsSync(ARCHIVE))('the baked Baden-Württemberg archive', () => {
    async function open() {
        const file = readFileSync(ARCHIVE);
        const pmtiles = new PMTiles({ getKey: () => ARCHIVE, getBytes: async (offset, length) => ({ data: file.buffer.slice(file.byteOffset + offset, file.byteOffset + offset + length) }) });
        const header = await pmtiles.getHeader();
        const requests: string[] = [];
        const archive: TileArchive = {
            metadata: await pmtiles.getMetadata() as Record<string, unknown>, minZoom: header.minZoom, maxZoom: header.maxZoom,
            bounds: [header.minLon, header.minLat, header.maxLon, header.maxLat],
            async get(z, x, y) { requests.push(`${z}/${x}/${y}`); return (await pmtiles.getZxy(z, x, y))?.data; },
        };
        return { source: climateSource(archive), requests };
    }

    it('has overview means of the detail years', async () => {
        const { source } = await open();
        expect(source.meta).toMatchObject({ firstYear: 2016, years: 10, wetDayMm: 2.3 });
        const freiburg = cellAt([7.86, 47.99]);
        const [o, d] = ([OVERVIEW, DETAIL] as const).map(level => locate(level, freiburg));
        const overviewTile = (await source.tile(OVERVIEW, o.x, o.y))!, detail = cellHistory((await source.tile(DETAIL, d.x, d.y))!, d.index);
        for (const week of [2, 28]) {
            const years = Array.from({ length: 10 }, (_, year) => detail.tmax[52 * year + week]);
            expect(Math.abs(read(overviewTile, 'tmax', week, o.index) - years.reduce((a, b) => a + b) / 10)).toBeLessThanOrEqual(0.25);
        }
        expect(read(overviewTile, 'tmax', 28, o.index)).toBeGreaterThan(read(overviewTile, 'tmax', 2, o.index) + 15);
        expect(windMode(windRose(overviewTile, o.index, 0))).toBeDefined();
    });

    it('serves a 100 km route from two overview tiles', async () => {
        const { source, requests } = await open();
        // Stuttgart, Tübingen, Hechingen, Balingen, Rottweil, Villingen.
        const { line, km } = densify([[9.18, 48.78], [9.06, 48.52], [8.96, 48.35], [8.85, 48.27], [8.63, 48.17], [8.46, 48.06]]);
        expect(km.at(-1)).toBeGreaterThan(95);
        expect(km.at(-1)).toBeLessThan(105);
        const started = performance.now();
        const cells = await sampleLine(line, source.tile);
        const sampled = performance.now();
        const rows = weatherRows(cells, km);
        const weathered = performance.now();
        const wind = windRow(cells, km, lineBearings(line));
        const done = performance.now();
        expect(requests.length).toBe(2);
        expect(cells.every(Boolean)).toBe(true);
        expect(rows.high[28]).toBeGreaterThan(rows.low[28]);
        expect([...wind].every(chance => chance > 0 && chance < 1)).toBe(true);
        await sampleLine(line, source.tile, DETAIL);
        console.info(`${line.length} samples, ${km.at(-1)!.toFixed(0)} km: overview 2 requests, detail ${requests.length - 2}; sample ${(sampled - started).toFixed(1)} ms, weather ${(weathered - sampled).toFixed(1)} ms, wind ${(done - weathered).toFixed(1)} ms`);
    });
});
