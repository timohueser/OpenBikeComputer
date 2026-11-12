/** Turns a ride object of the device into its samples and its library record. */
import { dateName, rideKeyAt } from '../library/import';
import type { Bike, RideRecord } from '../library/records';
import { rideSummary, type RideSample } from '../library/summary';
import type { RideObject } from '../usb/objects';

/** A sample of a device ride, with the sensor values; null where the sensor gave none. */
export interface DeviceSample extends RideSample {
    hrBpm: number | null;
    cadenceRpm: number | null;
    powerW: number | null;
}

/** The bike types of the device, by their number in the ride footer. */
const deviceBikes: Bike[] = ['road', 'gravel', 'mtb', 'touring'];

/** The samples of a ride object. The device and `RideSample` both store positions in microdegrees (`specs/ble-interface.md`
 * §7.2), so the positions copy through. The time of a sample is the start time plus its milliseconds after the first
 * sample, modulo 2³² because the clock of the device wraps there. */
export function deviceSamples(ride: RideObject): DeviceSample[] {
    const first = ride.points[0]?.tMs ?? 0;
    return ride.points.map(point => ({
        timeS: ride.startTime + ((point.tMs - first) >>> 0) / 1000,
        lat: point.latMicrodegrees,
        lon: point.lonMicrodegrees,
        elevationM: point.elevationM,
        segmentStart: point.segmentStart,
        hrBpm: point.hrBpm,
        cadenceRpm: point.cadenceRpm,
        powerW: point.powerW,
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
    };
}
