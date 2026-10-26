import { describe, expect, it } from 'vitest';
import { follow, groundOf, type Footing, type Probe } from './camera';

describe('the camera', () => {
    it('keeps the eye on a slope with a probe two frames late, fades a step down, rises at once and takes a new place at once', () => {
        for (const [speed, degrees] of [[6, 30], [12, 40]]) {
            const rise = Math.tan(degrees * Math.PI / 180), ground = (east: number) => 1200 + rise * east;
            const probe = (frame: number): Probe => ({ east: speed * frame / 60, north: 0, height: ground(speed * frame / 60), slope: [rise, 0], frame });
            let footing: Footing | undefined, worst = 0;
            for (let frame = 2; frame < 120; frame++) {
                const east = speed * frame / 60;
                footing = follow(footing, probe(frame - 2), east, 0, 1 / 60);
                worst = Math.max(worst, Math.abs(groundOf(footing, east, 0) - ground(east)));
            }
            expect(worst).toBeLessThan(1e-3);
        }

        const at = (height: number, frame: number): Probe => ({ east: 0, north: 0, height, slope: [0, 0], frame });
        let footing = follow(undefined, at(1200, 1), 0, 0, 0.016);
        expect(groundOf(footing, 0, 0)).toBe(1200);
        footing = follow(footing, at(1201, 2), 0, 0, 0.016);
        expect(groundOf(footing, 0, 0)).toBe(1201);
        for (let frame = 3; frame < 6; frame++) footing = follow(footing, at(1200, 3), 0, 0, 0.016);
        expect(groundOf(footing, 0, 0)).toBeGreaterThan(1200.2);
        expect(groundOf(footing, 0, 0)).toBeLessThan(1201);
        for (let frame = 6; frame < 66; frame++) footing = follow(footing, at(1200, 3), 0, 0, 0.016);
        expect(footing.step).toBe(0);
        expect(groundOf(follow(footing, at(2400, 3), 0, 0, 0.016), 0, 0)).toBe(2400);
    });
});
