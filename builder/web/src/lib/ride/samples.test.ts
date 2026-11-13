import { describe, expect, it } from 'vitest';
import { writeFitRide, type FitSample } from '../core/fit';
import { writeGpx } from '../core/gpx';
import type { RideRecord } from '../library/records';
import { encodeRideObject } from '../usb/objects';
import { ridePoints, rideSamples } from './samples';

const T = 1_792_828_800;
const record = (format: RideRecord['format']) => ({ name: 'Ride', format }) as RideRecord;

describe('rideSamples', () => {
    it('reads a FIT ride with its sensors', async () => {
        const samples: FitSample[] = [
            { timeS: T, lat: 46_900_000, lon: 8_400_000, segmentStart: true, elevationM: 1000, hrBpm: 120, cadenceRpm: 80, powerW: 200 },
            { timeS: T + 1, lat: 46_900_050, lon: 8_400_000, segmentStart: false, elevationM: 1001, hrBpm: 121 },
        ];
        const file = new Blob([writeFitRide({ sport: 'bike', samples }) as Uint8Array<ArrayBuffer>]);
        expect(await rideSamples(record('fit'), file)).toEqual(samples);
    });

    it('reads a GPX ride with its times, segments and sensors, and ridePoints writes them back', async () => {
        const points = [
            { lat: 46_900_000, lon: 8_400_000, ele: 1000, time: T, segmentStart: true as const, hrBpm: 142, cadenceRpm: 88, powerW: 250.5, temperatureC: 21.5 },
            { lat: 46_900_050, lon: 8_400_000, time: T + 1 },
            { lat: 46_900_100, lon: 8_400_000, time: T + 300, segmentStart: true as const, powerW: 0 },
        ];
        const samples = await rideSamples(record('gpx'), new Blob([writeGpx({ name: 'Ride', waypoints: [], points })]));
        expect(samples).toEqual([
            { timeS: T, lat: 46_900_000, lon: 8_400_000, elevationM: 1000, segmentStart: true, hrBpm: 142, cadenceRpm: 88, powerW: 250.5, temperatureC: 21.5 },
            { timeS: T + 1, lat: 46_900_050, lon: 8_400_000, segmentStart: false },
            { timeS: T + 300, lat: 46_900_100, lon: 8_400_000, segmentStart: true, powerW: 0 },
        ]);
        expect(ridePoints([...samples, { ...samples[1], speedMps: 5 }])).toEqual([...points, points[1]]);
    });

    it('reads a ride of the device, with no key for a sensor that gave no value', async () => {
        const file = new Blob([encodeRideObject({
            version: 6, isDemo: false, name: '', startTime: T, distanceM: 0, movingTimeS: 0, avgSpeedCms: 0, climbM: 0, descentM: 0,
            avgHr: null, maxHr: null, avgCadence: null, avgPower: null, maxPower: null, energyKj: null, bikeType: 1, trip: null,
            effortLimits: { maxHrBpm: 186, ftpW: 240 },
            points: [0, 1].map(i => ({ tMs: 5000 + i * 1500, latMicrodegrees: 46_900_000 + i * 50, lonMicrodegrees: 8_400_000, elevationM: 1000,
                segmentStart: !i, hrBpm: 130, cadenceRpm: null, powerW: i ? 210 : null })),
        }) as Uint8Array<ArrayBuffer>]);
        expect(await rideSamples(record('obcride'), file)).toEqual([
            { timeS: T, lat: 46_900_000, lon: 8_400_000, elevationM: 1000, segmentStart: true, hrBpm: 130 },
            { timeS: T + 1.5, lat: 46_900_050, lon: 8_400_000, elevationM: 1000, segmentStart: false, hrBpm: 130, powerW: 210 },
        ]);
    });
});
