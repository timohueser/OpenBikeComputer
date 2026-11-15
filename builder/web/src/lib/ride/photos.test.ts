import { describe, expect, it } from 'vitest';
import type { FitSample } from '../core/fit';
import { browserZone, photoSample, placePhoto } from './photos';

const start = Date.UTC(2026, 9, 24, 7) / 1000;
/** 60 samples 10 s apart, about 50 m each: 30 to the north, then back to the south on the same line. */
const samples: FitSample[] = Array.from({ length: 60 }, (_, i) => ({ timeS: start + 10 * i, lat: 46_900_000 + 450 * (i < 30 ? i : 59 - i), lon: 8_000_000, segmentStart: !i }));
const placeOf = (sample: number): [number, number] => [8, samples[sample].lat / 1e6];
const utc = () => 0;

describe('the place of a photo on a ride', () => {
    it('takes the clock time with the zone of the file, and the sample nearest to it', () => {
        const at = placePhoto(samples, { clock: start + 7200 + 204, zone: 7200, gpsTime: start + 999_999 }, utc);
        expect(at).toEqual({ time: start + 204 });
        expect(photoSample(samples, at!)).toBe(20);
    });

    it('takes the GPS time for a clock with no zone, else the zone of the browser', () => {
        expect(placePhoto(samples, { clock: start + 7200 + 100, gpsTime: start + 100 }, utc)).toEqual({ time: start + 100 });
        expect(placePhoto(samples, { clock: start + 3600 + 100 }, () => 3600)).toEqual({ time: start + 100 });
    });

    it('leaves out a photo before the start, after the end or with no time and no place', () => {
        expect(placePhoto(samples, { clock: start - 1, zone: 0 }, utc)).toBeUndefined();
        expect(placePhoto(samples, { clock: start + 591, zone: 0 }, utc)).toBeUndefined();
        expect(placePhoto(samples, {}, utc)).toBeUndefined();
    });

    it('takes the hour that puts a photo with no zone nearest to its GPS place', () => {
        // The camera clock is two hours ahead of the zone of the browser; the place is that of sample 40.
        expect(placePhoto(samples, { clock: start + 7200 + 400, place: placeOf(40) }, utc)).toEqual({ time: start + 400 });
        // The clock is also a minute fast: the right hour puts the photo 300 m from its place, and still wins.
        expect(placePhoto(samples, { clock: start + 7200 + 460, place: placeOf(40) }, utc)).toEqual({ time: start + 460 });
        // No hour is in the ride.
        expect(placePhoto(samples, { clock: start + 1800 + 400, place: placeOf(40) }, utc)).toBeUndefined();
    });

    it('takes no other hour for a photo whose GPS place is far from the line', () => {
        // Seven hours after the ride, 3 km east of it: the hour that is in the ride does not make it a photo of the ride.
        expect(placePhoto(samples, { clock: start + 7 * 3600 + 400, place: [8.04, 46.91] }, utc)).toBeUndefined();
        // In the ride by the zone of the browser, the photo keeps that time, far place or not.
        expect(placePhoto(samples, { clock: start + 400, place: [8.04, 46.91] }, utc)).toEqual({ time: start + 400 });
    });

    it('reads the zone of the browser on a day with daylight time and on a day with none', () => {
        const zone = process.env.TZ;
        process.env.TZ = 'Europe/Zurich';
        try {
            expect(browserZone(Date.UTC(2026, 6, 1, 12) / 1000)).toBe(7200);
            expect(browserZone(Date.UTC(2026, 0, 15, 12) / 1000)).toBe(3600);
        } finally {
            if (zone === undefined) delete process.env.TZ;
            else process.env.TZ = zone;
        }
    });

    it('puts a photo with no time at the first pass of the line within 100 m of its GPS place', () => {
        // Sample 49 is exactly at this place on the way back; sample 10 is the first pass.
        const at = placePhoto(samples, { place: placeOf(49) }, utc);
        expect(at).toEqual({ time: null, place: placeOf(49) });
        expect(photoSample(samples, at!)).toBe(10);
        expect(placePhoto(samples, { place: [8.002, 46.9] }, utc)).toBeUndefined();
    });

    it('places the photos of a ride with no times by their GPS place only', () => {
        const timeless = samples.map(sample => ({ ...sample, timeS: 0 }));
        expect(placePhoto(timeless, { clock: start + 100, zone: 0, place: placeOf(5) }, utc)).toEqual({ time: null, place: placeOf(5) });
        expect(placePhoto(timeless, { clock: start + 100, zone: 0 }, utc)).toBeUndefined();
    });
});
