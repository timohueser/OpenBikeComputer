import { describe, expect, it } from 'vitest';
import type { FitSample } from '../core/fit';
import { rideSummary, lineCoordinates } from '../library/summary';
import { heartRateZones } from '../settings/zones';
import { distanceAxis, extent, lineShares, range, sampleAt, smooth, speeds, thin, zoneStretches } from './lines';

const T = 1_792_828_800;
/** Microdegrees of latitude for about 50 m. */
const STEP = 450;
const sample = (seconds: number, steps: number, rest: Partial<FitSample> = {}): FitSample =>
    ({ timeS: T + seconds, lat: 46e6 + steps * STEP, lon: 8e6, segmentStart: false, ...rest });

describe('smooth', () => {
    it('takes the mean over 10 s, keeps a missing value missing and does not reach across a pause', () => {
        const samples = [0, 1, 2, 3, 4, 5, 6, 600, 601].map(seconds => sample(seconds, 0));
        const values = [100, 100, 100, 160, undefined, 100, 100, 300, 300];
        const smoothed = smooth(samples, values, 10);
        // The window of the fourth sample holds the six values of the first seven samples.
        expect(smoothed[3]).toBeCloseTo(110);
        expect(smoothed[4]).toBeUndefined();
        expect(smoothed[6]).toBeCloseTo(112);
        expect(smoothed[7]).toBe(300);
    });
});

describe('thin', () => {
    it('gives at most one value for each column: the mean of its samples, null with none', () => {
        const xs = Array.from({ length: 36_000 }, (_, i) => i / 35_999 * .5);
        const columns = thin(xs, xs.map((_, i) => i % 2 ? 10 : 20), 800);
        expect(columns).toHaveLength(800);
        expect(columns[0]).toBeCloseTo(15);
        expect(columns[399]).toBeCloseTo(15, 0);
        // The samples end at the middle of the axis, where the last one stands alone in its column.
        expect(columns[400]).toBe(10);
        expect(columns[401]).toBeNull();
        expect(range(columns)!.low).toBe(10);
        // A ride with few points: the columns between two samples join them; a sample with no value keeps its gap.
        expect(thin([0, .5, .75, .99], [10, 30, undefined, 50], 8)).toEqual([10, 15, 20, 25, 30, null, null, 50]);
        // A flat line draws in a range of 1; its smallest and its largest value are the same.
        expect(range([null, 7, null])).toEqual({ low: 7, high: 8 });
        expect(extent([null, 7, undefined, 7])).toEqual({ low: 7, high: 7 });
        expect(extent([3, undefined, 9, 5])).toEqual({ low: 3, high: 9 });
        expect(range([null])).toBeUndefined();
        expect(extent([undefined, null])).toBeUndefined();
    });
});

describe('zoneStretches', () => {
    it('splits a line where the zone changes and where a value is missing, with a smoothed value under an edge in the lower zone', () => {
        // Zones of a maximum of 200: Z2 from 120, Z3 from 140, Z4 from 160.
        const zones = heartRateZones(200);
        expect(zoneStretches([100, 119.9, 120, 139, null, 139.5, 140, 165, 150, null, null, 190], zones)).toEqual([
            { zone: 0, from: 0, to: 1 }, { zone: 1, from: 2, to: 3 },
            { zone: 1, from: 5, to: 5 }, { zone: 2, from: 6, to: 6 }, { zone: 3, from: 7, to: 7 }, { zone: 2, from: 8, to: 8 },
            { zone: 4, from: 11, to: 11 },
        ]);
        expect(zoneStretches([null, null], zones)).toEqual([]);
    });
});

describe('the axis of a ride with a pause that covers distance', () => {
    // 1 km, a pause that hops 5 km straight on, then 1 km: the distance axis is 2 km long, the summary line 7 km, and
    // the line keeps no point at the hop.
    const samples = [
        ...Array.from({ length: 21 }, (_, i) => sample(10 * i, i, { segmentStart: !i })),
        ...Array.from({ length: 21 }, (_, i) => sample(900 + 10 * i, 120 + i, { segmentStart: !i })),
    ];

    it('leaves the hop out of the distance and of the speed', () => {
        const { xs, lengthM } = distanceAxis(samples);
        expect(lengthM).toBeCloseTo(2000, -2);
        expect(xs[20]).toBeCloseTo(.5);
        expect(xs[21]).toBeCloseTo(.5);
        expect(xs.at(-1)).toBe(1);
        const speed = speeds(samples.map((s, i) => i === 5 ? { ...s, speedMps: 9 } : s));
        expect(speed[0]).toBeUndefined();
        expect(speed[1]).toBeCloseTo(5, 1);
        expect(speed[5]).toBe(9);
        expect(speed[21]).toBeUndefined();
    });

    it('puts each sample on the summary line, which measures the hop', () => {
        const line = lineCoordinates(rideSummary(samples, 'gravel'));
        const shares = lineShares(samples, line);
        expect(shares[0]).toBe(0);
        expect(shares[10]).toBeCloseTo(.5 / 7, 2);
        expect(shares[20]).toBeCloseTo(1 / 7, 2);
        expect(shares[21]).toBeCloseTo(6 / 7, 2);
        expect(shares.at(-1)).toBe(1);
        // A place in the hop is the first sample after it.
        expect(sampleAt(shares, .5)).toBe(21);
        expect(sampleAt(shares, 2)).toBe(41);
    });

    it('ends the line at the last sample, for a ride that passes its end, turns and comes back to it', () => {
        const back = [0, 20, 10, 20].map((steps, i) => sample(i * 60, steps));
        expect(lineShares(back, [[8, 46], [8, 46.009]])).toEqual([0, .5, .75, 1]);
    });
});
