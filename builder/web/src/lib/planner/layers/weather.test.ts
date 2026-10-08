import { describe, expect, it } from 'vitest';
import { DETAIL, OVERVIEW, WEEKS, climateTile, type Level } from './climate';
import {
    NO_TEMPERATURE, highAt, mapPlaces, nightLabels, paintWeather, placeLabels, rainRange, rainWeeks, rampColor, spread, stripClasses, stripScale, temperatureClass,
    weatherChart, weatherYear, weekLabel, word, type CellBlock, type Samples,
} from './weather';

// Plane order and value counts of specs/planner-climate-tiles.md; orography is the only 2-byte plane.
const PLANES: Record<Level, [string, number][]> = {
    [OVERVIEW]: [['orography', 1], ['lapse_tmax', 12], ['lapse_tmin', 12], ['rose', 192], ['wet_share', 52], ['rain', 52], ['tmax', 52], ['tmin', 52], ['wind', 52]],
    [DETAIL]: [['orography', 1], ['lapse_tmax', 12], ['lapse_tmin', 12], ['wet_days', 520], ['rain', 520], ['tmax', 520], ['tmin', 520], ['wind', 520]],
};

/** A tile with every value missing, and a writer of raw codes. */
function tile(level: Level) {
    const cells = level === OVERVIEW ? 384 : 96, starts = new Map<string, number>();
    let size = 0;
    for (const [name, count] of PLANES[level]) { starts.set(name, size); size += count * cells * (name === 'orography' ? 2 : 1); }
    const body = new Uint8Array(size).fill(0x80);
    for (let c = 0; c < cells; c++) body[2 * c + 1] = 0x80, body[2 * c] = 0;
    for (const name of ['rose', 'wet_share', 'wet_days', 'rain', 'wind']) {
        const at = starts.get(name);
        if (at !== undefined) body.fill(255, at, at + PLANES[level].find(p => p[0] === name)![1] * cells);
    }
    const view = new DataView(body.buffer);
    return {
        set(name: string, index: number, cell: number, code: number) {
            const at = starts.get(name)!;
            if (name === 'orography') view.setInt16(at + 2 * cell, code, true);
            else view.setUint8(at + index * cells + cell, code & 255);
        },
        tile: () => climateTile(level, 0, 0, body),
    };
}

/** Pixel words that are the scale index, so a test reads the painted value back. */
const INDEX = Uint32Array.from({ length: 256 }, (_, i) => i);
const temperatureIndex = (celsius: number) => word('temperature', INDEX, celsius);

describe('weather colours', () => {
    it('mixes the two nearest stops and keeps the end colours outside the scale', () => {
        expect(rampColor('temperature', 'light', -10)).toBe('#6f8fb3');
        expect(rampColor('temperature', 'light', 45)).toBe(rampColor('temperature', 'light', 30));
        // 5 °C is halfway between the stops at 0 and 10 °C.
        expect(rampColor('temperature', 'light', 5)).toBe('#cfd7d6');
        expect(rampColor('rain', 'dark', 0)).toBe('#2d2c22');
        expect(word('rain', INDEX, NaN)).toBe(0);
    });

    it('classes temperatures in whole degrees, with a no-data class', () => {
        expect([temperatureClass(-25), temperatureClass(0.4), temperatureClass(-60), temperatureClass(NaN)]).toEqual([0, 25, 0, NO_TEMPERATURE]);
    });
});

describe('weather map tiles', () => {
    // Two by two cells: 10 and 20 °C on top at 1000 m, 0 °C below, and one cell without data.
    const block: CellBlock = {
        col: 100, row: 50, cols: 2, rows: 2,
        value: Float32Array.of(10, 20, 0, NaN), lapse: Float32Array.of(-6.5, -6.5, -6.5, 0), orography: Float32Array.of(1000, 1000, 1000, 0),
    };
    const paint = (cols: number[], rows: number[], heights?: Float32Array) => {
        const words = new Uint32Array(256 * 256);
        const cells = { cols: Float64Array.from({ length: 256 }, (_, i) => cols[i] ?? 0), rows: Float64Array.from({ length: 256 }, (_, i) => rows[i] ?? 0) };
        paintWeather(words, cells, block, heights, 'temperature', INDEX);
        return words;
    };

    it('blends the cell centres, so the colours change smoothly between cells', () => {
        const words = paint([100, 100.5, 101], [50]);
        expect([words[0], words[1], words[2]]).toEqual([10, 15, 20].map(temperatureIndex));
    });

    it('moves each cell temperature to the terrain height of the pixel', () => {
        const heights = new Float32Array(256 * 256).fill(2000);
        expect(paint([100], [50], heights)[0]).toBe(temperatureIndex(3.5));
    });

    it('ends the data at the cell edge', () => {
        // Below the middle of the lower row, the missing cell holds more than half the weight.
        const words = paint([100.9, 100.9], [50.4, 50.9]);
        expect(words[0]).not.toBe(0);
        expect(words[256]).toBe(0);
    });
});

describe('weather along a route', () => {
    const week = 19, date = '2026-05-16';
    // Two overview cells at 500 m: highs 14 and 12 °C, lows 4 °C, wet on 30 % of the days.
    const overview = tile(OVERVIEW);
    for (const cell of [0, 1]) {
        overview.set('orography', 0, cell, 500);
        overview.set('lapse_tmax', 4, cell, -65);
        overview.set('lapse_tmin', 4, cell, -50);
        overview.set('wet_share', week, cell, 30);
        overview.set('tmin', week, cell, 8);
        overview.set('rain', week, cell, 13);
    }
    overview.set('tmax', week, 0, 28);
    overview.set('tmax', week, 1, 24);
    const detail = tile(DETAIL);
    detail.set('orography', 0, 0, 500);
    detail.set('lapse_tmin', 4, 0, -50);
    // Night lows over ten years: 1 to 10 °C, so the spread without the extreme years is 2–9 °C.
    for (let year = 0; year < 10; year++) detail.set('tmin', year * WEEKS + week, 0, 2 * (year + 1));
    const samples = (elevation: number[]): Samples => {
        const o = overview.tile(), d = detail.tile();
        return { overview: [{ tile: o, index: 0 }, { tile: o, index: 1 }], detail: i => i ? null : { tile: d, index: 0 }, elevation: Float32Array.from(elevation), km: [0, 10] };
    };

    it('has a High, a Low and a Rain row and names the week in its label', () => {
        const year = weatherYear(samples([500, 500]), date, 'light', 2016);
        expect(year.rows.map(row => row.label)).toEqual(['High', 'Low', 'Rain']);
        // The only rain week of the archive is the typical rain of the weeks around it.
        expect(year.label).toBe('On the route, 14–20 May: highs 12–14 °C · lows 4 °C · rain on 2.1 of 7 days, 10–20 mm');
        expect(year.rows[0].cells[week]).toBe(temperatureClass(13));
        expect(year.rows[1].cells[week]).toBe(temperatureClass(4));
        expect(year.fills[year.rows[2].cells[week]].label).toBe('2 of 7 days');
        expect(year.rows[0].cells[0]).toBe(NO_TEMPERATURE);
        expect(weatherYear(null, date, 'light', 2016).rows[0].cells.every(cell => cell === 255)).toBe(true);
    });

    it('gives each row its value in every week for the slider', () => {
        const rows = weatherYear(samples([500, 500]), date, 'light', 2016).rows;
        expect(rows.map(row => row.values![week])).toEqual(['13°', '4°', '2/7 d · 10–20 mm']);
        expect(rows.map(row => row.values![week + 2])).toEqual(['–', '–', '–']);
    });

    it('stretches the strip colours over the highs of the route and marks its coldest and warmest stretch', () => {
        const highs = Float32Array.of(9, 9.2, 12, 19, 18.8, 18.7, NaN, 14);
        const classes = stripClasses(highs);
        expect([classes[0], classes[3], classes[6]]).toEqual([0, 31, 32]);
        expect(stripScale(highs)).toEqual({ range: '9–19 °C along the route', marks: [{ index: 0, label: '9°' }, { index: 4, label: '19°' }] });
        // One temperature along the whole route takes the middle colour and needs no marks.
        expect([...stripClasses([5, 5])]).toEqual([16, 16]);
        // A route within 4 °C keeps near-even colours: 5.0 and 5.2 °C are neighbouring steps, not the ramp ends.
        expect([...stripClasses([5, 5.2])]).toEqual([15, 16]);
        expect([...stripClasses([5, 9])]).toEqual([0, 31]);
        expect(stripScale([5, 5.2])!.marks).toEqual([]);
        expect(stripScale([NaN])).toBeNull();
    });

    it('charts the wet days of the week for rain, and highs and lows for temperature', () => {
        const rainy = tile(DETAIL);
        // Wet days 1 to 5 over the years, two years each, so the extreme years do not narrow the spread. Week 51 has 8 days, 9 in a leap year.
        for (let year = 0; year < 10; year++) {
            rainy.set('wet_days', year * WEEKS + week, 0, 1 + Math.floor(year / 2));
            rainy.set('wet_days', year * WEEKS + 51, 0, 8);
            rainy.set('rain', year * WEEKS + week, 0, 10);
            // A dry week elsewhere makes this week twice the mean week.
            rainy.set('rain', year * WEEKS + 30, 0, 0);
        }
        const weeks = rainWeeks({ tile: rainy.tile(), index: 0 }, 2016);
        expect([weeks.mean[week], weeks.low[week], weeks.high[week]]).toEqual([3, 1, 5]);
        // 2016, 2020 and 2024 are leap years.
        expect(weeks.mean[51]).toBeCloseTo((7 * 7 + 3 * 7 * 8 / 9) / 10);
        const point = { ...samples([500]), detail: () => ({ tile: rainy.tile(), index: 0 }) };
        const rain = weatherChart(point, 0, 2016, date, 'light', 'rain');
        expect(rain.headline).toBe('14–20 May: rain on about 3 of 7 days (1–5 in most years) · 5–15 mm a week');
        expect(rain.grids).toEqual([]);
        expect(weatherChart(samples([500, 500]), 0, 2016, date, 'light', 'temperature').grids.map(grid => grid.label)).toEqual(['Daytime high', 'Night low']);
    });

    it('labels each overnight stop with the night lows of its week at its height', () => {
        expect(nightLabels(samples([500, 500]), [{ index: 0, date }, { index: 1, date }])).toEqual(['Night 2–9 °C', '']);
        // 1000 m higher at −5 K/km: 5 °C colder.
        expect(nightLabels(samples([1500, 500]), [{ index: 0, date }])).toEqual(['Night −3 to 4 °C']);
    });
});

describe('weather map labels', () => {
    const feature = (id: number, properties: Record<string, unknown>, coordinates = [8, 48]) => ({ id, properties, geometry: { type: 'Point', coordinates } });

    it('takes towns, villages and named peaks once each, with the basemap names', () => {
        const places = mapPlaces([
            feature(1, { kind: 'locality', name: 'Freiburg im Breisgau', 'name:en': 'Freiburg', population: 220286, population_rank: 10, min_zoom: 7 }),
            feature(1, { kind: 'locality', name: 'Freiburg im Breisgau', 'name:en': 'Freiburg', population: 220286, population_rank: 10, min_zoom: 7 }),
            feature(2, { kind: 'summit', name: 'Feldberg', elevation: 1494, min_zoom: 12 }),
            feature(3, { kind: 'summit', elevation: 1306 }),
            feature(4, { kind: 'neighbourhood', name: 'Wiehre' }),
        ]);
        expect(places.map(place => [place.name, place.peak, place.elevation])).toEqual([['Freiburg', false, undefined], ['Feldberg', true, 1494]]);
    });

    it('ranks towns, then named peaks, then villages, and labels a peak only with a value', () => {
        const places = mapPlaces([
            feature(1, { kind: 'locality', name: 'Eschbach', population: 2000 }),
            feature(2, { kind: 'summit', name: 'Feldberg', elevation: 1494 }),
            feature(3, { kind: 'locality', name: 'Titisee-Neustadt', population: 12000 }),
            feature(4, { kind: 'summit', name: 'Belchen', elevation: 1414 }),
        ]);
        const labels = placeLabels(places, ['18°', '9°', undefined, undefined]).features.map(({ properties }) => properties);
        expect(labels.map(label => label.name)).toEqual(['Eschbach', 'Feldberg', 'Titisee-Neustadt']);
        const byRank = [...labels].sort((a, b) => a.sort - b.sort);
        expect(byRank.map(label => [label.name, label.value])).toEqual([['Titisee-Neustadt', undefined], ['Feldberg', '9°'], ['Eschbach', '18°']]);
    });

    it('reads the high at a place as the map colours it, at the place height', () => {
        // Four cells at 10 °C on 1000 m with −6.5 K/km; a place at 2000 m between their centres.
        const t = tile(OVERVIEW);
        for (const cell of [0, 1, 24, 25]) {
            t.set('orography', 0, cell, 1000);
            t.set('lapse_tmax', 4, cell, -65);
            t.set('tmax', 19, cell, 20);
        }
        const decoded = t.tile();
        const cell = (col: number, row: number) => col < 2 && row < 2 ? { tile: decoded, index: row * 24 + col } : undefined;
        // Cell column 0 is centred on 180° W and row 0 on 90° N.
        expect(highAt([-179.95, 89.95], 2000, cell, 19)).toBeCloseTo(3.5);
        expect(highAt([-179.95, 89.95], NaN, cell, 19)).toBeCloseTo(10);
        expect(Number.isNaN(highAt([-170, 80], 500, cell, 19))).toBe(true);
    });

    it('gives the typical rain as a range two 5 mm steps wide', () => {
        expect([0, 2.4, 2.6, 7.4, 12.4, 13, 17.4, 42].map(mm => rainRange(mm))).toEqual(['0–5', '0–5', '0–10', '0–10', '5–15', '10–20', '10–20', '35–45']);
        expect(rainRange(13, '-')).toBe('10-20');
    });
});

describe('weather words', () => {
    it('drops the extreme years from a spread of five or more', () => {
        expect(spread([5, 1, 3, NaN, 9, 7])).toEqual([3, 7]);
        expect(spread([2, 4])).toEqual([2, 4]);
        expect(spread([NaN])).toBeNull();
    });

    it('names the days of the climate week', () => {
        expect(weekLabel('2026-05-16')).toBe('14–20 May');
        expect(weekLabel('2026-04-30')).toBe('30 Apr – 6 May');
        expect(weekLabel('2026-12-31')).toBe('24–31 Dec');
    });
});
