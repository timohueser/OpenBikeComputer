/** The personal text of the Home page: the open plan, else the rides of this year, else the slogan. */
import type { RideRecord, RouteRecord } from '../../lib/library/records';
import { yearTotals, type Totals } from '../../lib/library/statistics';

export type HomeText =
    | { kind: 'continue'; route: RouteRecord; ago: string }
    | { kind: 'year'; totals: Totals; last: RideRecord; hike: boolean; ago: string }
    | { kind: 'slogan' };

/** The open plan shows only when it changed in this time. */
const CONTINUE_MS = 7 * 86_400_000;

/** "today", "yesterday", "4 days ago", else the date. By the local calendar. */
export function ago(at: number, now: Date): string {
    const day = (date: Date) => Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000;
    const then = new Date(at), days = day(now) - day(then);
    if (days <= 0) return 'today';
    if (days === 1) return 'yesterday';
    if (days < 14) return `${days} days ago`;
    const year = then.getFullYear() !== now.getFullYear();
    return `on ${then.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', ...year && { year: 'numeric' } })}`;
}

/** An imported route is never the open plan, so an import never shows "Continue". */
export function homeText(routes: RouteRecord[], rides: RideRecord[], active: string | null | undefined, now: Date): HomeText {
    const open = active ? routes.find(route => route.id === active) : undefined;
    if (open?.summary && now.getTime() - open.updatedAt < CONTINUE_MS) return { kind: 'continue', route: open, ago: ago(open.updatedAt, now) };
    const totals = yearTotals(rides, now.getFullYear());
    if (totals.rides + totals.hikes) {
        const last = rides.reduce((newest, ride) => ride.summary.start > newest.summary.start ? ride : newest);
        return { kind: 'year', totals, last, hike: last.summary.bike === 'hiking', ago: ago(last.summary.start * 1000, now) };
    }
    return { kind: 'slogan' };
}
