/** Finds the trips of the rides: the rides of one trip on the device, and the rides that go on from where the ride before ended. */
import type { RideRecord } from './records';
import { distanceM } from './summary';

/** The rides of a trip in start order, and the day of each, from 1. A trip has rides on 2 days or more. */
export interface Trip {
    /** The trip key of the device, else the key of the first ride. */
    id: string;
    name: string;
    rides: RideRecord[];
    days: number[];
}

const JOIN_M = 5000, HOME_M = 20_000, BASE_M = 5000, GAP_DAYS = 2;

/** The local day of a Unix time in seconds, as a count of days. */
export function localDay(seconds: number): number {
    const date = new Date(seconds * 1000);
    return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000;
}

const first = (ride: RideRecord) => ride.summary.line.slice(0, 2);
const last = (ride: RideRecord) => ride.summary.line.slice(-2);
const metres = ([lon1, lat1]: number[], [lon2, lat2]: number[]) => distanceM(lon1, lat1, lon2, lat2);

/**
 * A device ride with a trip belongs to that trip, on its day. Each other ride joins the trip of the ride before when it
 * starts within 5 km of that ride's end, on the same local day as its start or at most 2 days later, and when each night
 * of the trip is more than 20 km from the trip's first point: a commute, loops from one base and a train day make no
 * trip. A run of 3 days or more whose nights are all within 5 km of one place is a commute or a base, and no trip.
 */
export function trips(rides: RideRecord[]): Trip[] {
    const sorted = [...rides].sort((a, b) => a.summary.start - b.summary.start);
    const out: Trip[] = [], device = new Map<string, Trip>();
    let chain: RideRecord[] = [];
    const close = () => {
        const day0 = chain.length ? localDay(chain[0].summary.start) : 0, days = chain.map(ride => localDay(ride.summary.start) - day0 + 1);
        const nights = chain.filter((_, i) => days[i + 1] > days[i]).map(last);
        const base = days.at(-1)! > 2 && nights.every(night => metres(night, nights[0]) <= BASE_M);
        if (days.at(-1)! > 1 && !base) out.push({ id: chain[0].key, name: chain[0].name, rides: chain, days });
        chain = [];
    };
    for (const ride of sorted) {
        if (ride.trip) {
            const trip = device.get(ride.trip.key) ?? { id: ride.trip.key, name: ride.trip.name || ride.name, rides: [], days: [] };
            device.set(trip.id, trip);
            trip.rides.push(ride);
            trip.days.push(ride.trip.day);
            continue;
        }
        const before = chain.at(-1);
        const later = before ? localDay(ride.summary.start) - localDay(before.summary.start) : 0;
        const joins = before && later >= 0 && later <= GAP_DAYS && metres(last(before), first(ride)) <= JOIN_M
            && (!later || metres(last(before), first(chain[0])) > HOME_M);
        if (!joins) close();
        chain.push(ride);
    }
    close();
    for (const trip of device.values()) if (new Set(trip.days).size > 1) out.push(trip);
    return out.sort((a, b) => a.rides[0].summary.start - b.rides[0].summary.start);
}
