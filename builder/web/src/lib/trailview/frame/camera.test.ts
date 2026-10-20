import { describe, expect, it } from 'vitest';
import { eyeAltitude, MAX_ALTITUDE, move, NEAR, toggle, viewProjection, WALK_EYE, zoom, type Pose } from './camera';

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
});
