/** Finds the trips of the rides: the rides of one trip on the device, and the rides that go on from where the ride before ended. */
import type { RideRecord } from './records';

/** The rides of a trip in start order, and the day of each, from 1. A trip has rides on 2 days or more. */
export interface Trip {
    /** The trip key of the device, else the key of the first ride. */
    id: string;
    name: string;
    rides: RideRecord[];
    days: number[];
}

// Start values, to test on real rides.
const JOIN_M = 5000, HOME_M = 20_000, GAP_DAYS = 2;
const METRES_PER_DEGREE = 6_371_008.8 * Math.PI / 180;

/** The local day of a Unix time in seconds, as a count of days. */
export function localDay(seconds: number): number {
    const date = new Date(seconds * 1000);
    return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000;
}

const first = (ride: RideRecord) => ride.summary.line.slice(0, 2);
const last = (ride: RideRecord) => ride.summary.line.slice(-2);
const end = (ride: RideRecord) => ride.summary.start + ride.summary.seconds;

function metres([lon1, lat1]: number[], [lon2, lat2]: number[]): number {
    const rad = Math.PI / 180, h = Math.sin((lat2 - lat1) * rad / 2) ** 2
        + Math.cos(lat1 * rad) * Math.cos(lat2 * rad) * Math.sin((lon2 - lon1) * rad / 2) ** 2;
    return 2 * METRES_PER_DEGREE * 180 / Math.PI * Math.asin(Math.min(1, Math.sqrt(h)));
}

/**
 * A device ride with a trip belongs to that trip, on its day. Each other ride joins the trip of the ride before when it
 * starts within 5 km of that ride's end, on the same local day or at most 2 days later, and when each night of the trip
 * is more than 20 km from the trip's first point: a commute, loops from one base and a train day make no trip.
 */
export function trips(rides: RideRecord[]): Trip[] {
    const sorted = [...rides].sort((a, b) => a.summary.start - b.summary.start);
    const out: Trip[] = [], device = new Map<string, Trip>();
    let chain: RideRecord[] = [];
    const close = () => {
        const day0 = chain.length ? localDay(chain[0].summary.start) : 0, days = chain.map(ride => localDay(ride.summary.start) - day0 + 1);
        if (days.at(-1)! > 1) out.push({ id: chain[0].key, name: chain[0].name, rides: chain, days });
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
        const later = before ? localDay(ride.summary.start) - localDay(end(before)) : 0;
        const joins = before && later >= 0 && later <= GAP_DAYS && metres(last(before), first(ride)) <= JOIN_M
            && (!later || metres(last(before), first(chain[0])) > HOME_M);
        if (!joins) close();
        chain.push(ride);
    }
    close();
    for (const trip of device.values()) if (new Set(trip.days).size > 1) out.push(trip);
    return out.sort((a, b) => a.rides[0].summary.start - b.rides[0].summary.start);
}
