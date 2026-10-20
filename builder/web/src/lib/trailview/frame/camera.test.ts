import { describe, expect, it } from 'vitest';
import { eyeAltitude, follow, groundOf, MAX_ALTITUDE, move, NEAR, toggle, viewProjection, WALK_EYE, zoom, type Footing, type Pose, type Probe } from './camera';

const walk: Pose = { east: 0, north: 0, heading: 90, pitch: 0, mode: 'walk', height: WALK_EYE };

describe('the camera', () => {
    it('projects with reversed Z: depth is 1 at the near plane and falls to 0 at infinity', () => {
        const m = viewProjection(90, 0, 1.5);
        const clip = (x: number, y: number, z: number) => [0, 1, 2, 3].map(r => m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r]);
        for (const distance of [NEAR, 10, 1e6]) {
            const [x, y, z, w] = clip(distance, 0, 0);
            expect(x / w).toBeCloseTo(0, 9);
            expect(y / w).toBeCloseTo(0, 9);
            expect(z / w).toBeCloseTo(NEAR / distance, 6);
        }
    });

    it('walks with the eye 1.6 m above the ground, in the direction of the heading', () => {
        const step = move(walk, { ahead: 1, right: 0, up: 0, run: false }, 1, 1200);
        expect(step.east).toBeCloseTo(6, 9);
        expect(step.north).toBeCloseTo(0, 9);
        expect(eyeAltitude(step, 1200)).toBeCloseTo(1201.6, 9);
        expect(eyeAltitude(step, undefined)).toBeUndefined();
    });

    it('zooms out from a walk to at least 60 km altitude, and walks again on the ground', () => {
        let pose = walk;
        for (let i = 0; i < 40; i++) pose = zoom(pose, 300, 1000);
        expect(pose.mode).toBe('fly');
        expect(pose.height).toBe(MAX_ALTITUDE);
        expect(MAX_ALTITUDE).toBeGreaterThanOrEqual(60_000);
        expect(toggle(pose, 1000)).toEqual({ ...walk, height: WALK_EYE });
    });

    it('keeps the eye on a slope at walking and running speed, with a probe two frames late', () => {
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
    });

    it('fades a step between two probes out where the eye stands, and takes a new place at once', () => {
        const at = (height: number, frame: number): Probe => ({ east: 0, north: 0, height, slope: [0, 0], frame });
        let footing = follow(undefined, at(1200, 1), 0, 0, 0.016);
        expect(groundOf(footing, 0, 0)).toBe(1200);
        for (let frame = 2; frame < 5; frame++) footing = follow(footing, at(1201, 2), 0, 0, 0.016);
        expect(groundOf(footing, 0, 0)).toBeGreaterThan(1200.2);
        expect(groundOf(footing, 0, 0)).toBeLessThan(1201);
        for (let frame = 5; frame < 65; frame++) footing = follow(footing, at(1201, 2), 0, 0, 0.016);
        expect(footing.step).toBe(0);
        expect(groundOf(follow(footing, at(2400, 3), 0, 0, 0.016), 0, 0)).toBe(2400);
    });
});
