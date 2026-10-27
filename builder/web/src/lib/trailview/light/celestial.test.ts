import { describe, expect, it } from 'vitest';
import { solarPosition, solarTerms } from '../../planner/layers/sun';
import { moonAt, moonEcliptic, starFrame } from './celestial';

const RAD = Math.PI / 180;
const angle = (a: number[], b: number[]) => Math.acos(Math.min(1, a[0] * b[0] + a[1] * b[1] + a[2] * b[2])) / RAD;

describe('the sky about a place', () => {
    it('puts the moon where Meeus puts it', () => {
        // Example 47.a: 1992 April 12, 0 h.
        const moon = moonEcliptic(Date.UTC(1992, 3, 12));
        expect(moon.longitude).toBeCloseTo(133.162655, 2);
        expect(moon.latitude).toBeCloseTo(-3.229126, 2);
        // The cut series leaves out distance terms of up to 30 km.
        expect(Math.abs(moon.km - 368409.7)).toBeLessThan(100);
    });

    it('covers the sun with the moon in Dallas during the eclipse of 8 April 2024', () => {
        // Totality in Dallas lasted from 18:40:43 to 18:44:34 UTC; the drawn disks are 0.27° wide.
        const place: [number, number] = [-96.797, 32.777], instant = Date.UTC(2024, 3, 8, 18, 42, 40);
        const { altitude, bearing } = solarPosition(place, solarTerms(instant));
        const sun = [Math.sin(bearing) * Math.cos(altitude), Math.cos(bearing) * Math.cos(altitude), Math.sin(altitude)];
        expect(angle(moonAt(place, instant), sun)).toBeLessThan(0.05);
    });

    it('turns the stars about the pole', () => {
        // The pole of the date stands at the latitude's height due north, 0.15° from that of J2000 in 2026: 20″ a year.
        const frame = starFrame([8.3, 46.8], Date.UTC(2026, 9, 27, 22)), pole = [frame[2], frame[5], frame[8]];
        expect(angle(pole, [0, Math.cos(46.8 * RAD), Math.sin(46.8 * RAD)])).toBeCloseTo(0.15, 1);
    });
});
