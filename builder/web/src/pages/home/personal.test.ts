import { describe, expect, it } from 'vitest';
import type { RideRecord, RouteRecord, RouteSummary } from '../../lib/library/records';
import { ago, homeText } from './personal';

const now = new Date(2026, 10, 13, 18);
const DAY = 86_400_000;
const route = (id: string, updatedAt: number, summary: Partial<RouteSummary> | null = {}) =>
    ({ id, name: id, updatedAt, summary: summary && { lengthM: 96_000, climbM: 1_200, days: 2, ...summary } }) as RouteRecord;
const ride = (key: string, start: Date, bike = 'gravel') => ({ key, name: key, summary: { start: start.getTime() / 1000, lengthM: 62_000, bike } }) as RideRecord;

describe('homeText', () => {
    const plan = route('plan', now.getTime() - 2 * DAY), rides = [ride('older', new Date(2026, 3, 1)), ride('newest', new Date(2026, 10, 9))];

    it('shows the open plan first, then the rides of this year, then the slogan', () => {
        expect(homeText([plan], rides, 'plan', now)).toMatchObject({ kind: 'continue', route: { id: 'plan' }, ago: '2 days ago' });
        expect(homeText([plan], rides, null, now)).toMatchObject({ kind: 'year', totals: { lengthM: 124_000, rides: 2, hikes: 0 }, last: { key: 'newest' }, ago: '4 days ago' });
        expect(homeText([plan], [ride('last year', new Date(2025, 11, 31))], null, now)).toEqual({ kind: 'slogan' });
    });

    it('names the last record a hike when it was on foot', () => {
        expect(homeText([], rides, null, now)).toMatchObject({ hike: false });
        expect(homeText([], [...rides, ride('walk', new Date(2026, 10, 12), 'hiking')], null, now)).toMatchObject({ last: { key: 'walk' }, hike: true, totals: { rides: 2, hikes: 1 } });
    });

    it('does not continue a plan older than 7 days, a plan with no line, or an import', () => {
        // An import is dated at the import, but it is not the open plan.
        expect(homeText([route('plan', now.getTime() - 8 * DAY), route('import', now.getTime())], [], 'plan', now).kind).toBe('slogan');
        expect(homeText([route('plan', now.getTime(), null)], [], 'plan', now).kind).toBe('slogan');
    });
});

describe('ago', () => {
    it('counts calendar days, and gives the date after two weeks', () => {
        expect([ago(new Date(2026, 10, 13, 0, 5).getTime(), now), ago(new Date(2026, 10, 12, 23, 55).getTime(), now),
            ago(new Date(2026, 9, 31).getTime(), now), ago(new Date(2026, 9, 30).getTime(), now), ago(new Date(2025, 9, 30).getTime(), now)])
            .toEqual(['today', 'yesterday', '13 days ago', 'on 30 Oct', 'on 30 Oct 2025']);
    });
});
