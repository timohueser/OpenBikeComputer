/** The totals of the rides over a time. */
import type { RideRecord } from './records';

export interface Totals {
    /** The length of the rides and the hikes. */
    lengthM: number;
    /** The count by bike. */
    rides: number;
    /** The count on foot. */
    hikes: number;
}

/** The totals of the rides that start in `year`, local time. */
export function yearTotals(rides: RideRecord[], year: number): Totals {
    const totals: Totals = { lengthM: 0, rides: 0, hikes: 0 };
    for (const { summary } of rides) {
        if (new Date(summary.start * 1000).getFullYear() !== year) continue;
        totals.lengthM += summary.lengthM;
        if (summary.bike === 'hiking') totals.hikes++;
        else totals.rides++;
    }
    return totals;
}
