// Landmarks: the buildings a place is known by, drawn as themselves rather than as a kit house, from the
// prototype's src/models/landmarks.ts. Footprint-sized, base at the origin, z up.
import { rgb } from '../structures/mesh';
import { footing, Model, random, shade } from './flat';

const STONE = ['#8a877e', '#7c7a72', '#969287'].map(h => rgb(h)), SLIT = rgb('#2c2a28');

/** A roofless stone tower on a width × depth footprint, its wall tops broken to uneven heights, with a few dark slits. */
export function ruin(width: number, depth: number, height: number, seed: number): Model {
    const m = new Model(), rand = random(seed), [hx, hy] = [width / 2, depth / 2], ring = [[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]], deep = footing(Math.hypot(hx, hy));
    ring.forEach(([ax, ay], side) => {
        const [bx, by] = ring[(side + 1) % 4], steps = Math.max(2, Math.round(Math.hypot(bx - ax, by - ay) / 1.4));
        for (let s = 0; s < steps; s++) {
            const [t0, t1] = [s / steps, (s + 1) / steps], [x0, y0, x1, y1] = [ax + (bx - ax) * t0, ay + (by - ay) * t0, ax + (bx - ax) * t1, ay + (by - ay) * t1];
            // Mostly whole to the wall head, broken down here and there; a storey at a time, so a slit can sit in one.
            const top = height * (rand() < 0.75 ? 1 - 0.04 * rand() : 0.7 + 0.2 * rand()), stone = STONE[side % STONE.length];
            m.poly([[x0, y0, -deep], [x1, y1, -deep], [x1, y1, 0], [x0, y0, 0]], stone);
            for (let z = 0; z < top; z += 3) {
                const z1 = Math.min(z + 3, top), slit = z > 1 && z1 < top - 1 && s === Math.floor(steps / 2) && rand() < 0.6;
                m.poly([[x0, y0, z], [x1, y1, z], [x1, y1, z1], [x0, y0, z1]], slit ? SLIT : shade(stone, 0.94 + 0.08 * rand()));
            }
        }
    });
    return m;
}

/** A squat white lighthouse: a tapering tower, a dark lantern and a cap, on the middle of its footprint. */
export function lighthouse(width: number, depth: number, height: number): Model {
    const r = Math.max(1.5, Math.min(width, depth) / 2), m = new Model(), dark = rgb('#2f3a40');
    m.beam([0, 0, -footing(r)], [0, 0, height], 2 * r, rgb('#f2efe6'), { sides: 12, w1: 1.6 * r });
    m.beam([0, 0, height], [0, 0, height + 0.3], 2.1 * r, dark, { sides: 12, capped: true });
    m.beam([0, 0, height + 0.3], [0, 0, height + 1.9], 1.1 * r, rgb('#3a4a50'), { sides: 10 });
    m.beam([0, 0, height + 1.9], [0, 0, height + 2.9], 1.3 * r, dark, { sides: 10, w1: 0.01, capped: true });
    return m;
}
