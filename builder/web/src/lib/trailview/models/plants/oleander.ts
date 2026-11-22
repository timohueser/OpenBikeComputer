import type { Plant } from '../plant-library';
import { across, domeAt, domed, hash, leaf, merge, onDome, paint, place, ramp, random, relight, revolve, sheet, smooth, soft, spindle, tufts, UP, v3, type Colour, type Geometry, type Ramp, type Tuft } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Oleander leaves: dark, narrow and leathery, the same all year. */
const LEAVES: Ramp = ['#1a2c1e', '#2c4830', '#5a7a52'];

/** Pink flowers on most bushes, white on one in five. */
const PINK: Ramp = ['#a83460', '#d8608c', '#f29ab8'];
const WHITE: Ramp = ['#b8b4ac', '#e8e4dc', '#faf8f2'];

/** The sides of the middle dome and of a side dome, their stem tips in leaf, and the trusses of the bush by season. */
const SIDES = [7, 5], TIPS = [6, 4], TRUSSES = 22, BLOOM: Record<Season, number> = { winter: 0, spring: 9, summer: TRUSSES, autumn: 9 };

/** A dome of the bush: narrow at its foot, widest below its middle and round on top, a little knobbly, lit as an egg. */
function dome([x, y, r, h]: Tuft, sides: number, [deep, mid]: Colour[], seed: number): Geometry {
    const egg = soft(revolve([[0.72 * r, 0], [r, 0.38 * h], [0.72 * r, 0.8 * h], [0, h]], sides), 0.08 * r, 4 / r, seed);
    relight(egg, (p) => v3(p.x / r, p.y / r, (p.z - 0.3 * h) / h).normalize().lerp(UP, 0.3).normalize());
    return paint(place(egg, { at: [x, y, -0.03], rot: [0, 0, seed] }), (p) => deep.clone().lerp(mid, smooth(0.1, 0.9, p.z / h)));
}

/**
 * Oleander: a tall bush of four dark domes, the middle one the tallest, with a pair of long narrow leaves that
 * stand up and out at each stem tip. In flower a truss, a round pink or white bud, sits between the leaves of a
 * tip, and in summer on every tip and on four stems more. 182 triangles, 314 in full bloom.
 */
function oleander(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LEAVES);
    const [shade, petal, lit] = ramp(hash(v3(seed, 1, 2), 5) < 0.2 ? WHITE : PINK);
    const cards = sheet();
    const mounds = tufts([0, 0, 0.4, 0.95], 3, 0.8, [0.34, 0.3, 0.6], [0.06, 0.06, 0.2], rand);
    const parts = mounds.map((mound, k) => dome(mound, SIDES[k ? 1 : 0], [deep.clone().multiplyScalar(0.8), mid.clone().lerp(tip, 0.15)], seed + k));
    let nth = 0;
    mounds.forEach((mound, k) => {
        const [x, y, r, h] = mound, light = domed(v3(x, y, 0), v3(r, r, h), 0.3);
        const turn = rand() * 2 * Math.PI, sun = rand() < 0.3 ? 0.5 : 0.2 * rand();
        // The middle dome has four stems more, which carry a truss in full bloom but no leaves.
        for (let j = 0; j < TIPS[k ? 1 : 0] + (k ? 0 : 4); j++, nth++) {
            // A side dome carries its tips on its top and its open flank, where the middle dome does not hide them.
            const [at, out] = k ? domeAt(mound, Math.atan2(y, x) + (j - 1.5) * (0.7 + 0.2 * rand()), j % 3 ? 0.35 : 1.05) : onDome(mound, j, 10, 1.3, turn);
            const stem = out.clone().addScaledVector(UP, 0.7).normalize(), [u] = across(stem), spin = rand() * Math.PI;
            if (j < TIPS[k ? 1 : 0]) for (const e of [1, -1]) {
                const dir = stem.clone().addScaledVector(u.clone().applyAxisAngle(stem, spin), 0.7 * e).normalize();
                const body = mid.clone().lerp(tip, sun + 0.3 * rand());
                leaf(cards, at, dir, 0.38 + 0.08 * rand(), 0.12, 0.5, [deep, body, body.clone().lerp(tip, 0.5)], light);
            }
            // The tips flower in an order of their own, so a bush is the same in every season.
            if ((nth * 7) % TRUSSES < BLOOM[season]) spindle(cards, at, at.clone().addScaledVector(stem, 0.17), 0.085, 3, [shade, petal, lit]);
        }
    });
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Oleander',
    latin: 'Nerium oleander',
    build: (season, seed) => oleander(season, seed, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [2, 4], share: 0.01, own: true, clumped: true },
} satisfies Plant;
