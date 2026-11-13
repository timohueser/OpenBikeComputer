import { describe, expect, it } from 'vitest';
import type { FitSample } from '../core/fit';
import { channels, movingSpeed, partNumbers, rideClimbs, rideCsv } from './numbers';

const T = 1_792_828_800;
/** Microdegrees of latitude for about 50 m, so a step of 10 s moves at about 5 m/s. */
const STEP = 450;

describe('channels', () => {
    it('weighs the moving steps by their seconds, and leaves out a stop, a pause and a cadence of 0, also in the speed', () => {
        const samples: FitSample[] = [
            { timeS: T, lat: 0, lon: 8e6, segmentStart: true, hrBpm: 100 },
            { timeS: T + 10, lat: STEP, lon: 8e6, segmentStart: false, hrBpm: 120, cadenceRpm: 80, powerW: 200 },
            { timeS: T + 20, lat: 2 * STEP, lon: 8e6, segmentStart: false, hrBpm: 140, cadenceRpm: 0 },
            // A stop of one minute with a drift of half a metre, then a pause.
            { timeS: T + 80, lat: 2 * STEP + 5, lon: 8e6, segmentStart: false, hrBpm: 180 },
            { timeS: T + 600, lat: 3 * STEP, lon: 8e6, segmentStart: true, hrBpm: 190 },
            { timeS: T + 610, lat: 4 * STEP, lon: 8e6, segmentStart: false, hrBpm: 160, cadenceRpm: 90, powerW: 300 },
        ];
        const { hr, power, cadence } = channels(samples, { maxHr: 200, ftp: null });
        // Zones of a maximum of 200: Z2 from 120, Z3 from 140, Z4 from 160, Z5 from 180.
        expect(hr).toEqual({ mean: 140, max: 160, zones: expect.any(Array) });
        expect(hr!.zones!.map(zone => zone.seconds)).toEqual([0, 10, 10, 10, 0]);
        expect(power).toEqual({ mean: 250, max: 300 });
        expect(cadence).toEqual({ mean: 85, max: 90 });
        expect(movingSpeed(samples)).toBeCloseTo(5.004, 2);
        expect(channels(samples.map(({ powerW: _, ...rest }) => rest), { maxHr: null, ftp: 250 })).toMatchObject({ hr: { mean: 140 }, power: undefined });
    });
});

describe('partNumbers', () => {
    it('gives the numbers of a part with a pause in it, with no step into the part, the time in each zone and no power', () => {
        // 50 m every 10 s, 2 m up each step. The part is the samples 2 to 7; a pause stands before sample 5.
        const samples: FitSample[] = Array.from({ length: 10 }, (_, i) => ({
            timeS: T + 10 * i + (i >= 5 ? 600 : 0), lat: 46e6 + i * STEP, lon: 8e6, segmentStart: i === 0 || i === 5,
            elevationM: 500 + 2 * i, hrBpm: i < 5 ? 130 : 170, cadenceRpm: 80,
        }));
        const part = partNumbers(samples, [2, 7], 'gravel', { maxHr: 200, ftp: 250 });
        // The steps to the samples 3, 4, 6 and 7 move: 200 m in 40 s.
        expect(part.lengthM).toBeCloseTo(200, -1);
        expect(part.movingS).toBe(40);
        // The climb counts a rise of 5 m or more above the last low: 504 m to 510 m.
        expect(part.climbM).toBe(6);
        expect(part.speed).toBeCloseTo(5, 1);
        expect(part.channels.hr).toMatchObject({ mean: 150, max: 170 });
        // Zones of a maximum of 200: 130 is in Z2, 170 in Z4.
        expect(part.channels.hr!.zones!.map(zone => zone.seconds)).toEqual([0, 20, 0, 20, 0]);
        expect(part.channels.power).toBeUndefined();
        expect(samples[2].segmentStart).toBe(false);
    });
});

describe('rideClimbs', () => {
    it('finds the climbs of each segment, on the distance with no hop across a pause, with their moving time and mean power', () => {
        // 50 m every 10 s: down 20 m in 2 km, up 180 m in 3 km; a pause that hops 2 km and 300 m up, as on a train; then
        // down 20 m in 1 km, up 120 m in 1.5 km, flat 1 km.
        const samples: FitSample[] = [{ timeS: T, lat: 46e6, lon: 8e6, segmentStart: true, elevationM: 500, powerW: 100 }];
        const ride = (length: number, rise: number) => {
            for (let m = 50; m <= length; m += 50) {
                const last = samples.at(-1)!;
                samples.push({ timeS: last.timeS + 10, lat: last.lat + STEP, lon: 8e6, segmentStart: false, elevationM: last.elevationM! + rise * 50 / length, powerW: rise > 0 ? 250 : 100 });
            }
        };
        ride(2000, -20);
        ride(3000, 180);
        const before = samples.at(-1)!;
        samples.push({ ...before, timeS: before.timeS + 1800, lat: before.lat + 40 * STEP, elevationM: before.elevationM! + 300, segmentStart: true });
        ride(1000, -20);
        ride(1500, 120);
        ride(1000, 0);
        const climbs = rideClimbs(samples);
        // The climbs of the profile; the tolerance leaves room for a later change of the climb rule.
        expect(climbs.map(({ from, to, ascent }) => [from, to, ascent])).toEqual([
            [expect.closeTo(2, 0), expect.closeTo(5, 0), expect.closeTo(180, -1)],
            [expect.closeTo(6, 0), expect.closeTo(7.5, 0), expect.closeTo(120, -1)],
        ]);
        for (const climb of climbs) {
            // The climb rule measures the gain on heights that pass a 3 m dead band.
            expect(samples[climb.last].elevationM! - samples[climb.first].elevationM!).toBeCloseTo(climb.ascent, -1);
            // About 5 m/s on the distance of the climb.
            expect(climb.seconds / ((climb.to - climb.from) * 1000 / 5)).toBeCloseTo(1, 1);
            expect(climb.powerW).toBeGreaterThan(200);
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
