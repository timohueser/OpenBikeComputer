/** The totals of the rides over a time and the largest values, by bike and on foot apart. */
import { bikes, type Bike, type RideRecord } from './records';
import { localDay, tripDays, type Trip } from './trips';

export interface Totals {
    lengthM: number;
    climbM: number;
    movingS: number;
    count: number;
}
export type Measure = keyof Totals;

/** The totals of one bar, and of each kind of bike in it. */
export interface Part {
    totals: Totals;
    kinds: Map<Bike, Totals>;
}

export interface Largest {
    /** The longest ride. */
    ride?: RideRecord;
    /** The local day with the most climb, and the ride of that day with the most climb. */
    day?: { climbM: number; ride: RideRecord };
    /** The trip with the most distance. `rides` are only its rides of this statistic, on 2 days or more, and `lengthM` and
     * `days` are of them. */
    trip?: { trip: Trip; rides: RideRecord[]; lengthM: number; days: number };
    /** The ride with the highest point. */
    highest?: RideRecord;
}

/** The numbers by bike and on foot are on different scales: no total holds a ride of each. */
export const onFoot = (ride: RideRecord) => ride.summary.bike === 'hiking';

const started = (ride: RideRecord) => new Date(ride.summary.start * 1000);

/** The rides that start in `year`, local time. */
export const inYear = (rides: RideRecord[], year: number) => rides.filter(ride => started(ride).getFullYear() === year);

export function sum(rides: RideRecord[]): Totals {
    const totals = { lengthM: 0, climbM: 0, movingS: 0, count: rides.length };
    for (const { summary } of rides) {
        totals.lengthM += summary.lengthM;
        totals.climbM += summary.climbM;
        totals.movingS += summary.movingS;
    }
    return totals;
}

/** The totals of one year by bike, and on foot. */
export function yearTotals(rides: RideRecord[], year: number): { bike: Totals; foot: Totals } {
    const all = inYear(rides, year);
    return { bike: sum(all.filter(ride => !onFoot(ride))), foot: sum(all.filter(onFoot)) };
}

/** The totals of each kind of bike that has a ride. */
export function byKind(rides: RideRecord[]): Map<Bike, Totals> {
    return new Map(bikes.map(bike => [bike, sum(rides.filter(ride => ride.summary.bike === bike))] as const).filter(([, totals]) => totals.count));
}

const part = (rides: RideRecord[]): Part => ({ totals: sum(rides), kinds: byKind(rides) });

/** One part for each year, from the first year with a ride to `thisYear`. A year with no ride has its place. */
export function byYear(rides: RideRecord[], thisYear: number): (Part & { year: number })[] {
    if (!rides.length) return [];
    const years = rides.map(ride => started(ride).getFullYear()), first = Math.min(...years);
    return Array.from({ length: Math.max(thisYear, ...years) - first + 1 }, (_, i) => ({ year: first + i, ...part(inYear(rides, first + i)) }));
}

/** The 12 months of `year`. */
export function byMonth(rides: RideRecord[], year: number): Part[] {
    const all = inYear(rides, year);
    return Array.from({ length: 12 }, (_, month) => part(all.filter(ride => started(ride).getMonth() === month)));
}

const most = <T>(items: T[], value: (item: T) => number) =>
    items.reduce<T | undefined>((best, item) => best === undefined || value(item) > value(best) ? item : best, undefined);

/**
 * The largest values of `rides`, which are the rides of one statistic, or of those in `year`. A trip counts with its rides
 * of this statistic only, of each year, for the year of the first of them.
 */
export function largest(rides: RideRecord[], trips: Trip[], year?: number): Largest {
    const shown = year === undefined ? rides : inYear(rides, year), keys = new Set(rides.map(ride => ride.key));
    const days = new Map<number, RideRecord[]>();
    for (const ride of shown) days.set(localDay(ride.summary.start), [...days.get(localDay(ride.summary.start)) ?? [], ride]);
    const day = most([...days.values()], day => sum(day).climbM);
    const own = trips.map(trip => ({ trip, rides: trip.rides.filter(ride => keys.has(ride.key)) }))
        .filter(({ rides }) => rides.length && (year === undefined || started(rides[0]).getFullYear() === year))
        .map(({ trip, rides }) => ({ trip, rides, lengthM: sum(rides).lengthM, days: tripDays(trip, rides) })).filter(({ days }) => days > 1);
    return {
        ride: most(shown, ride => ride.summary.lengthM),
        day: day && sum(day).climbM > 0 ? { climbM: sum(day).climbM, ride: most(day, ride => ride.summary.climbM)! } : undefined,
        trip: most(own, ({ lengthM }) => lengthM),
        highest: most(shown.filter(ride => ride.summary.highestM !== null), ride => ride.summary.highestM!),
    };
}
