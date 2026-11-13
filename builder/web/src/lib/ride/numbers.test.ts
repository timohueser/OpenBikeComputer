import { describe, expect, it } from 'vitest';
import type { FitSample } from '../core/fit';
import { channels, rideClimbs, rideCsv } from './numbers';

const T = 1_792_828_800;
/** Microdegrees of latitude for about 50 m, so a step of 10 s moves at about 5 m/s. */
const STEP = 450;

describe('channels', () => {
    it('weighs the moving steps by their seconds, and leaves out a stop, a pause and a cadence of 0', () => {
        const samples: FitSample[] = [
            { timeS: T, lat: 0, lon: 8e6, segmentStart: true, hrBpm: 100 },
            { timeS: T + 10, lat: STEP, lon: 8e6, segmentStart: false, hrBpm: 120, cadenceRpm: 80, powerW: 200 },
            { timeS: T + 20, lat: 2 * STEP, lon: 8e6, segmentStart: false, hrBpm: 140, cadenceRpm: 0 },
            // A stop of one minute, then a pause.
            { timeS: T + 80, lat: 2 * STEP, lon: 8e6, segmentStart: false, hrBpm: 180 },
            { timeS: T + 600, lat: 3 * STEP, lon: 8e6, segmentStart: true, hrBpm: 190 },
            { timeS: T + 610, lat: 4 * STEP, lon: 8e6, segmentStart: false, hrBpm: 160, cadenceRpm: 90, powerW: 300 },
        ];
        const { hr, power, cadence } = channels(samples, { maxHr: 200, ftp: null });
        // Zones of a maximum of 200: Z2 from 120, Z3 from 140, Z4 from 160, Z5 from 180.
        expect(hr).toEqual({ mean: 140, max: 160, zones: expect.any(Array) });
        expect(hr!.zones!.map(zone => zone.seconds)).toEqual([0, 10, 10, 10, 0]);
        expect(power).toEqual({ mean: 250, max: 300 });
        expect(cadence).toEqual({ mean: 85, max: 90 });
        expect(channels(samples.map(({ powerW: _, ...rest }) => rest), { maxHr: null, ftp: 250 })).toMatchObject({ hr: { mean: 140 }, power: undefined });
    });
});

describe('rideClimbs', () => {
    it('finds the climbs of the ride in their order, with their moving time and mean power', () => {
        // 50 m every 10 s: down 20 m in 2 km, up 180 m in 3 km, down 60 m in 2 km, up 120 m in 1.5 km, flat 1 km.
        const rises: [number, number][] = [[2000, -20], [3000, 180], [2000, -60], [1500, 120], [1000, 0]];
        const samples: FitSample[] = [{ timeS: T, lat: 46e6, lon: 8e6, segmentStart: true, elevationM: 500, powerW: 100 }];
        for (const [length, rise] of rises) for (let m = 50; m <= length; m += 50) {
            const last = samples.at(-1)!;
            samples.push({ timeS: last.timeS + 10, lat: last.lat + STEP, lon: 8e6, segmentStart: false, elevationM: last.elevationM! + rise * 50 / length, powerW: rise > 0 ? 250 : 100 });
        }
        const climbs = rideClimbs(samples);
        expect(climbs).toHaveLength(2);
        expect(climbs[0].to).toBeLessThan(climbs[1].from);
        for (const climb of climbs) {
            // About 5 m/s on the line of the climb.
            expect(climb.seconds / ((climb.to - climb.from) * 1000 / 5)).toBeCloseTo(1, 1);
            expect(climb.powerW).toBeGreaterThan(200);
            // Each climb has one grade.
            expect(climb.steepest).toBeCloseTo(climb.gradient, 0);
        }
    });
});

describe('rideCsv', () => {
    it('writes one line for each sample in SI units, skips the step to a segment start, and leaves a missing value empty', () => {
        const csv = rideCsv([
            { timeS: T, lat: 46_900_000, lon: 8_400_000, segmentStart: true, elevationM: 1000.04, speedMps: 5.1234, hrBpm: 120, temperatureC: 12 },
            { timeS: T + 10, lat: 46_900_450, lon: 8_400_000, segmentStart: false },
            { timeS: T + 600, lat: 46_901_000, lon: 8_400_000, segmentStart: true, powerW: 210, cadenceRpm: 85 },
        ]);
        expect(csv.split('\n')).toEqual([
            'time,latitude,longitude,elevation_m,distance_m,speed_m_s,heart_rate_bpm,cadence_rpm,power_w,temperature_c',
            '2026-10-24T08:00:00.000Z,46.900000,8.400000,1000,0,5.123,120,,,12',
            '2026-10-24T08:00:10.000Z,46.900450,8.400000,,50,,,,,',
            '2026-10-24T08:10:00.000Z,46.901000,8.400000,,50,,,85,210,',
            '',
        ]);
    });
});
