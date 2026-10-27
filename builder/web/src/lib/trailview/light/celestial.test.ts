import { describe, expect, it } from 'vitest';
import { moonAt, moonEcliptic, starFrame, sunAt } from './celestial';
import { sunlightLeft } from './eclipse';

const RAD = Math.PI / 180;
const angle = (a: number[], b: number[]) => Math.acos(Math.min(1, a[0] * b[0] + a[1] * b[1] + a[2] * b[2])) / RAD;

describe('the sky about a place', () => {
    it('puts the moon where Meeus puts it', () => {
        // Example 47.a: 1992 April 12, 0 h TT.
        const moon = moonEcliptic(Date.UTC(1992, 3, 12) - 69184);
        expect(moon.longitude).toBeCloseTo(133.162655, 4);
        expect(moon.latitude).toBeCloseTo(-3.229126, 4);
        expect(moon.km).toBeCloseTo(368409.7, 0);
    });

    it('casts the shadow of 12 August 2026 where NASA puts its path', () => {
        // Espenak's path table: the central line and the limits at 17:30 UT over Greenland and at 18:28 UT over Spain.
        const rows: [number, [number, number], [number, number], [number, number]][] = [
            [Date.UTC(2026, 7, 12, 17, 30), [-27.788, 73.683], [-23.145, 74.267], [-31.972, 73.043]],
            [Date.UTC(2026, 7, 12, 18, 28), [-6.188, 43.372], [-2.085, 42.908], [-9.552, 43.607]],
        ];
        for (const [instant, central, ...limits] of rows) {
            const at = (place: [number, number]) => {
                const sun = sunAt(place, instant), moon = moonAt(place, instant);
                return { apart: angle(sun.toward, moon.toward) * 3600, umbra: (moon.radius - sun.radius) / RAD * 3600 };
            };
            // 1″ moves the shadow by about 2 km; the umbra's radius is 30″ to 37″.
            expect(at(central).apart).toBeLessThan(3);
            for (const limit of limits) expect(Math.abs(at(limit).apart - at(limit).umbra)).toBeLessThan(2);
        }
    });

    it('leaves Madrid a sliver of the sun on 12 August 2026, and Burgos none', () => {
        const least = (place: [number, number]) => Math.min(...Array.from({ length: 600 }, (_, s) => {
            const instant = Date.UTC(2026, 7, 12, 18, 25, s);
            return sunlightLeft(sunAt(place, instant), moonAt(place, instant));
        }));
        expect(least([-3.70, 42.34])).toBe(0);
        expect(least([-3.70, 40.42])).toBeGreaterThan(0);
    });

    it('turns the stars about the pole', () => {
        // The pole of the date stands at the latitude's height due north, 0.15° from that of J2000 in 2026: 20″ a year.
        const frame = starFrame([8.3, 46.8], Date.UTC(2026, 9, 27, 22)), pole = [frame[2], frame[5], frame[8]];
        expect(angle(pole, [0, Math.cos(46.8 * RAD), Math.sin(46.8 * RAD)])).toBeCloseTo(0.15, 1);
    });
});
