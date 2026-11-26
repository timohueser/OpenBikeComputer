import type { Plant } from '../plant-library';
import { across, cushion, domeAt, hex, merge, onDome, paint, ramp, random, sheet, smooth, soft, tube, tufts, UP, v3, type Geometry, type Ramp, type V, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Mountain pine needles: dark green, a little olive in winter. */
const MUGO: Record<Season, Ramp> = {
    winter: ['#1c3024', '#2e4c34', '#5a7a4c'],
    spring: ['#1a3224', '#2c5236', '#62904e'],
    summer: ['#1a3024', '#2c4e36', '#5a884c'],
    autumn: ['#1a3024', '#2c4c34', '#58844a'],
};

/** Pale new shoots on the brush tips: colour and share, by season. */
const MUGO_CANDLES: Record<Season, [string, number]> = {
    winter: ['#4c6a42', 0],
    spring: ['#a8b878', 0.6],
    summer: ['#7a9a5a', 0.3],
    autumn: ['#4c6a42', 0],
};

/**
 * Mountain pine: a low mound of five dark domes, about twice as wide as tall, with needle brushes that turn up out
 * of them at the branch ends, and two crooked limbs that creep out from under it along the ground and turn up into
 * brushes of their own. A brush is a three-sided cup, a point at its foot and wide at its ragged top, closed by a
 * pale face, a candle in spring.
 */
function mountainPine(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(MUGO[season]);
    const [candle, share] = MUGO_CANDLES[season];
    const shoot = hex(candle);
    const [bark, barkLit] = [hex('#3a302a'), hex('#6e6052')];
    const cards = sheet();
    const mounds = tufts([0, 0, 0.5, 0.8], 4, 0.7, [0.5, 0.4, 0.5], [0.1, 0.08, 0.15], rand);
    const parts = mounds.map((mound, k) => cushion(mound, k ? 6 : 8, [deep.clone().multiplyScalar(0.7), deep, deep.clone().lerp(mid, 0.6)], 0.3, seed + k));
    // Brushes turn up at the branch ends, so they stand more upright than the dome they grow from.
    const brush = (foot: V, out: V, sun: number) => {
        const axis = out.clone().addScaledVector(UP, 1.2).normalize(), [u, w] = across(axis), spin = rand() * Math.PI;
        const body = mid.clone().lerp(tip, sun + 0.3 * rand());
        const top = rand() < share ? body.clone().lerp(shoot, 0.85) : body.clone().lerp(tip, 0.3);
        const rim = [0, 1, 2, 0].map((i) => {
            const a = spin + (i * 2 * Math.PI) / 3;
            return foot.clone().addScaledVector(axis, i === 1 ? 0.44 : 0.35).addScaledVector(u, 0.105 * Math.cos(a)).addScaledVector(w, 0.105 * Math.sin(a));
        });
        const round = (p: V) => p.clone().sub(foot).projectOnPlane(axis).normalize().lerp(UP, 0.4).normalize();
        for (let i = 0; i < 3; i++) cards.add([foot, rim[i], rim[i + 1]], [deep, body, body], round);
        cards.add(rim.slice(0, 3), [top, top, top], () => axis);
    };
    mounds.forEach((mound, k) => {
        const [x, y] = mound, turn = rand() * 2 * Math.PI, sun = rand() < 0.3 ? 0.4 : 0.15 * rand();
        // A side dome carries its brushes on its top and its open flank, where the middle dome does not hide them.
        for (let j = 0; j < (k ? 6 : 11); j++) brush(...(k ? domeAt(mound, Math.atan2(y, x) + (j - 2.5) * (0.5 + 0.1 * rand()), j % 2 ? 0.45 : 1.05) : onDome(mound, j, 11, 1.2, turn)), sun);
    });
    for (const [x, y, r] of [mounds[1], mounds[3]]) {
        const d = Math.hypot(x, y);
        const out = v3(x / d, y / d, 0);
        const bend = v3(-out.y, out.x, 0).multiplyScalar((rand() - 0.5) * 0.3);
        const end = out.clone().multiplyScalar(d + 0.95 * r).setZ(0.2);
        const path: Vec[] = [[0.2 * out.x, 0.2 * out.y, 0.03], out.clone().multiplyScalar(0.7 * d).add(bend).setZ(0.04).toArray(), out.clone().multiplyScalar(d + 0.85 * r).add(bend).setZ(0.05).toArray(), end.toArray()];
        parts.push(paint(soft(tube(path, 0.04, 0.6, 3, 3)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.3, p.z))));
        for (const side of [0.5, -0.5]) brush(end.clone().setZ(0.16), out.clone().applyAxisAngle(UP, side), 0.2);
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Mountain pine',
    latin: 'Pinus mugo',
    build: (season, seed) => mountainPine(season, seed, random(seed)),
    ground: { on: ['shrub'], height: [1, 2.5], share: 0.04, own: true },
} satisfies Plant;
