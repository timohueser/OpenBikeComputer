/** The rows of the ridden view: one for each trip, each group of rides on one road, and each other ride. */
import type { Trip } from '../../lib/library/trips';
import { rideItem, type RideItem } from './filter';

export interface Row {
    kind: 'ride' | 'group' | 'trip';
    /** The key of the ride, the id of the group or the id of the trip. */
    id: string;
    name: string;
    /** A trip: all its rides, in start order. Else the shown rides, newest first. */
    rides: RideItem[];
    /** A trip only: its days, rest days included. */
    days?: number;
    /** The newest start of the rides, in ms. */
    at: number;
}

/**
 * A ride is in one row: its trip, else a group of 2 or more shown rides, else its own. A trip row holds the whole trip
 * when one of its rides is shown. A group takes the name of its route, else of its newest ride. Newest first.
 */
export function rideRows(shown: RideItem[], trips: Trip[], groups: Map<string, string>, routeNames: Map<string, string>): Row[] {
    const tripOf = new Map(trips.flatMap(trip => trip.rides.map(ride => [ride.key, trip] as const)));
    const rows = new Map<string, Row>(), grouped = new Map<string, RideItem[]>();
    for (const item of [...shown].sort((a, b) => b.at - a.at)) {
        const trip = tripOf.get(item.id);
        if (trip) {
            if (!rows.has(`trip:${trip.id}`)) rows.set(`trip:${trip.id}`, { kind: 'trip', id: trip.id, name: trip.name,
                rides: trip.rides.map(rideItem), days: trip.days.at(-1)! - trip.days[0] + 1, at: trip.rides.at(-1)!.summary.start * 1000 });
            continue;
        }
        const group = groups.get(item.id) ?? item.id;
        grouped.set(group, [...grouped.get(group) ?? [], item]);
    }
    for (const [id, rides] of grouped) {
        if (rides.length === 1) rows.set(`ride:${rides[0].id}`, { kind: 'ride', id: rides[0].id, name: rides[0].name, rides, at: rides[0].at });
        else rows.set(`group:${id}`, { kind: 'group', id, name: routeNames.get(id) ?? rides[0].name, rides, at: rides[0].at });
    }
    return [...rows.values()].sort((a, b) => b.at - a.at);
}

/** "24 Oct 2026". */
export const dayText = (at: number, parts: Intl.DateTimeFormatOptions = { day: 'numeric', month: 'short', year: 'numeric' }) =>
    new Date(at).toLocaleDateString('en-GB', parts);

/** The dates of a trip, "3 to 9 Aug 2026", with the first month and year only where they differ. */
export function tripDates(row: Row): string {
    const [from, to] = [new Date(row.rides[0].at), new Date(row.rides.at(-1)!.at)];
    const year = from.getFullYear() !== to.getFullYear(), month = year || from.getMonth() !== to.getMonth();
    return `${dayText(from.getTime(), { day: 'numeric', ...month && { month: 'short' }, ...year && { year: 'numeric' } })} to ${dayText(to.getTime())}`;
}
