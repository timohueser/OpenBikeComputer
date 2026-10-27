import { describe, expect, it } from 'vitest';
import { daylight, instantAt, solarPosition, solarTerms } from './solar';

const RAD = Math.PI / 180;
describe('sun position and region clock', () => {
    it('matches geometric NREL SPA positions through the seasons', () => {
        for (const [date, bearing, altitude] of [
            ['2026-06-21T07:00:00Z', 89.545571, 32.042599],
            ['2026-12-21T11:00:00Z', 173.65359, 18.486956],
            ['2026-03-20T05:30:00Z', 89.04318, -1.074814],
        ] as const) {
            const position = solarPosition([7.95, 47.83], solarTerms(Date.parse(date)));
            expect(Math.abs(position.bearing / RAD - bearing)).toBeLessThan(.02);
            expect(Math.abs(position.altitude / RAD - altitude)).toBeLessThan(.02);
        }
    });
    it('uses the region DST rather than the computer timezone, and rejects a clock gap', () => {
        expect(new Date(instantAt('2026-06-21', 540, 'Europe/Berlin')).toISOString()).toBe('2026-06-21T07:00:00.000Z');
        expect(new Date(instantAt('2026-12-21', 540, 'Europe/Berlin')).toISOString()).toBe('2026-12-21T08:00:00.000Z');
        expect(() => instantAt('2026-03-29', 150, 'Europe/Berlin')).toThrow('does not exist');
    });
    it('spans the daylight in five-minute bins of the region clock', () => {
        const clock = (span: [number, number] | null) => span?.map(minute => `${Math.floor(minute / 60)}:${String(minute % 60).padStart(2, '0')}`);
        expect(clock(daylight([8.54, 47.37], '2026-06-21', 'Europe/Zurich'))).toEqual(['5:30', '21:25']);
        expect(clock(daylight([8.54, 47.37], '2026-12-21', 'Europe/Zurich'))).toEqual(['8:10', '16:40']);
        expect(daylight([18.96, 69.65], '2026-12-21', 'Europe/Oslo')).toBeNull();
    });
});
