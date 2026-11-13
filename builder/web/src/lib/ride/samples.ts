/** Reads the stored source file of a ride into its samples, and names the FIT sport of a bike. */
import { readFit, type FitSample, type FitSport } from '../core/fit';
import { readGpx, type GpxPoint } from '../core/gpx';
import { deviceSamples } from '../device-sync/ride';
import type { Bike, RideRecord } from '../library/records';
import { decodeRideObject } from '../usb/objects';

/** The samples of the source file of `ride`, by its format. It throws when the file cannot be read. */
export async function rideSamples(ride: RideRecord, file: Blob): Promise<FitSample[]> {
    if (ride.format === 'gpx') {
        return readGpx(await file.text(), ride.name).points.map(({ time, ele, segmentStart, ...rest }) => ({
            ...rest, timeS: time ?? 0, segmentStart: !!segmentStart, ...ele !== undefined && { elevationM: ele },
        }));
    }
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (ride.format === 'obcride') return deviceSamples(decodeRideObject(bytes));
    const read = readFit(bytes, ride.name);
    if (!('samples' in read)) throw new Error('The file is a course, not a ride.');
    return read.samples;
}

/** The GPX points of `samples`, with their times and sensors; GPX has no speed. */
export const ridePoints = (samples: FitSample[]): GpxPoint[] => samples.map(({ timeS, elevationM, segmentStart, speedMps: _, ...rest }) => ({
    ...rest, time: timeS, ...elevationM !== undefined && { ele: elevationM }, ...segmentStart && { segmentStart: true as const },
}));

const sports: Record<Bike, FitSport> = { road: 'road', gravel: 'gravel', mtb: 'mtb', touring: 'bike', hiking: 'foot' };

/** The FIT sport of a bike: touring is a plain ride by bike. */
export const fitSport = (bike: Bike): FitSport => sports[bike];
