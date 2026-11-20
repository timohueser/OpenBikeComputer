/** Finds the place of a photo on a ride: from its time against the times of the samples, else from its GPS place. */
import type { FitSample } from '../core/fit';
import type { PhotoFacts } from '../library/photo-time';
import type { PhotoEntry, RideSummary } from '../library/records';
import { distanceM } from '../library/summary';
import { firstIndex } from '../planner/geo';

/** A photo with no time is on the line when its GPS place is within NEAR_M of it. A clock with no zone can be up to
 * ZONE_HOURS from the zone of the browser. */
const NEAR_M = 100, ZONE_HOURS = 14;

/** What the record of a photo keeps of its place. */
export type PhotoPlace = Pick<PhotoEntry, 'time' | 'place'>;

const metresTo = (sample: FitSample, [lon, lat]: [number, number]) => distanceM(sample.lon / 1e6, sample.lat / 1e6, lon, lat);

/** The sample nearest to a place, in the first pass of the line within NEAR_M of the place. */
function sampleNear(samples: FitSample[], place: [number, number]): number | undefined {
    let found: number | undefined, least = NEAR_M;
    for (let i = 0; i < samples.length; i++) {
        const metres = metresTo(samples[i], place);
        if (metres <= least) [found, least] = [i, metres];
        else if (found !== undefined && metres > NEAR_M) break;
    }
    return found;
}

/** The sample nearest to a time, or undefined for a time outside the ride. */
function sampleAtTime(samples: FitSample[], time: number): number | undefined {
    if (time < samples[0].timeS || time > samples.at(-1)!.timeS) return undefined;
    const after = firstIndex(samples.length, i => samples[i].timeS >= time);
    return after && time - samples[after - 1].timeS < samples[after].timeS - time ? after - 1 : after;
}

/** The zone of the browser at a clock time, in seconds ahead of UTC. */
export function browserZone(clock: number): number {
    const at = new Date(clock * 1000);
    return clock - new Date(at.getUTCFullYear(), at.getUTCMonth(), at.getUTCDate(), at.getUTCHours(), at.getUTCMinutes(), at.getUTCSeconds()).getTime() / 1000;
}

/**
 * The place of a photo on a ride, as its record keeps it, or undefined for a photo that is not of the ride.
 * The time is the clock time with the zone of the file, else the GPS time, else the clock time in the zone of the
 * browser on that day. A clock time with no zone, and a GPS place within NEAR_M of the line, takes the whole hours from
 * the zone of the browser that put the photo nearest to that place; a photo with a place far from the line keeps the
 * zone of the browser. A time outside the ride is not of the ride. A photo with no time, and each photo of a ride with no
 * times, is of the ride when its GPS place is within NEAR_M of the line.
 */
export function placePhoto(samples: FitSample[], facts: PhotoFacts, zone = browserZone): PhotoPlace | undefined {
    const { clock, place } = facts;
    const byPlace = () => place && sampleNear(samples, place) !== undefined ? { time: null, place } : undefined;
    if (samples.at(-1)!.timeS <= samples[0].timeS) return byPlace();
    let time = clock !== undefined && facts.zone !== undefined ? clock - facts.zone : facts.gpsTime;
    if (time === undefined && clock !== undefined) {
        const local = time = clock - zone(clock);
        // The hour has no distance limit: a clock that is a minute off puts a moving rider some hundred metres away.
        if (place && sampleNear(samples, place) !== undefined) {
            const metresAt = (candidate: number) => {
                const sample = sampleAtTime(samples, candidate);
                return sample === undefined ? Infinity : metresTo(samples[sample], place);
            };
            let least = metresAt(local);
            // From the zone of the browser outwards, so that the nearer zone wins between two hours at the same distance.
            for (let hours = 1; hours <= ZONE_HOURS; hours++) {
                for (const candidate of [local - hours * 3600, local + hours * 3600]) {
                    const metres = metresAt(candidate);
                    if (metres < least) [time, least] = [candidate, metres];
                }
            }
        }
    }
    if (time === undefined) return byPlace();
    return sampleAtTime(samples, time) === undefined ? undefined : { time };
}

/** The sample of a stored photo, or undefined when the ride has no place for it. */
export const photoSample = (samples: FitSample[], photo: PhotoPlace): number | undefined =>
    photo.place ? sampleNear(samples, photo.place) : sampleAtTime(samples, photo.time!);

/** A ride that a photo can be of. `samples` reads its file. */
export interface PhotoRide { summary: RideSummary; samples: () => Promise<FitSample[] | undefined> }

/** The bounds of a line hold each place within NEAR_M of it when they grow by this many degrees, also far north. */
const NEAR_DEGREES = 0.01;

/** False for a ride whose times or bounds cannot give the photo a place, so that its file is not read. */
function canHold({ start, seconds, bounds: [west, south, east, north] }: RideSummary, { clock, gpsTime, place }: PhotoFacts): boolean {
    if (seconds <= 0 || (clock === undefined && gpsTime === undefined)) {
        return !!place && place[0] >= west - NEAR_DEGREES && place[0] <= east + NEAR_DEGREES && place[1] >= south - NEAR_DEGREES && place[1] <= north + NEAR_DEGREES;
    }
    const slack = clock === undefined ? 0 : ZONE_HOURS * 3600, time = clock ?? gpsTime!;
    return time >= start - slack && time <= start + seconds + slack;
}

/** The ride of a photo and its place on it by `placePhoto`, or undefined for a photo that fits no ride. Of two rides
 * that fit, the earlier one wins. */
export async function photoRide<R extends PhotoRide>(rides: R[], facts: PhotoFacts, zone = browserZone): Promise<{ ride: R; at: PhotoPlace } | undefined> {
    for (const ride of rides.filter(ride => canHold(ride.summary, facts)).sort((a, b) => a.summary.start - b.summary.start)) {
        const samples = await ride.samples(), at = samples?.length ? placePhoto(samples, facts, zone) : undefined;
        if (at) return { ride, at };
    }
    return undefined;
}
