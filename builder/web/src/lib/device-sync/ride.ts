/** Turns a ride object of the device into its samples and its library record. */
import type { FitSample } from '../core/fit';
import { dateName, rideKeyAt } from '../library/import';
import type { Bike, RideRecord } from '../library/records';
import { rideSummary } from '../library/summary';
import type { RideObject } from '../usb/objects';

/** The bike types of the device, by their number in the ride footer. */
const deviceBikes: Bike[] = ['road', 'gravel', 'mtb', 'touring'];

/** The samples of a ride object, with no key for a value that the sensor did not give. The device and `FitSample` both
 * store positions in microdegrees (`specs/ble-interface.md` §7.2), so the positions copy through. The time of a sample is
 * the start time plus its milliseconds after the first sample, modulo 2³² because the clock of the device wraps there. */
export function deviceSamples(ride: RideObject): FitSample[] {
    const first = ride.points[0]?.tMs ?? 0;
    return ride.points.map(({ tMs, latMicrodegrees, lonMicrodegrees, elevationM, segmentStart, hrBpm, cadenceRpm, powerW }) => ({
        timeS: ride.startTime + ((tMs - first) >>> 0) / 1000, lat: latMicrodegrees, lon: lonMicrodegrees, elevationM, segmentStart,
        ...hrBpm !== null && { hrBpm }, ...cadenceRpm !== null && { cadenceRpm }, ...powerW !== null && { powerW },
    }));
}

/** The library record of a ride object, with `device` set to `ref` (`storeId:objectId`). Null for a demo ride, a ride with no
 * points, and a ride that started before the device had a clock. */
export function rideRecord(ride: RideObject, ref: string): RideRecord | null {
    if (ride.isDemo || !ride.points.length || !ride.startTime) return null;
    const bike = deviceBikes[ride.bikeType], summary = rideSummary(deviceSamples(ride), bike);
    return {
        key: rideKeyAt(summary.start), name: ride.name || dateName(summary.start, bike), source: 'device', format: 'obcride', summary,
        maxHr: ride.effortLimits.maxHrBpm, ftp: ride.effortLimits.ftpW, device: ref,
        ...ride.trip && { trip: { key: ride.trip.key.toString(), day: ride.trip.dayIndex + 1, name: ride.trip.name } },
    };
}
