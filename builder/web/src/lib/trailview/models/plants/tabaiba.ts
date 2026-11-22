import type { Plant } from '../plant-library';
import { across, GOLDEN, hex, leaf, merge, paint, ramp, random, sheet, smooth, soft, tube, UP, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Tabaiba leaves: pale glaucous green, yellowing as the summer drought sheds them. */
const TABAIBA: Record<Season, Ramp> = {
    winter: ['#5a7a4a', '#8eb070', '#c4d896'],
    spring: ['#5a7a4a', '#92b272', '#c8da98'],
    summer: ['#7a7a44', '#aaa864', '#d2c88a'],
    autumn: ['#5e7a4a', '#90ae6e', '#c0d292'],
};

/** The twigs of each of the five limbs, and where each twig's tip lies on the dome: radians down from its top. */
const FORKS = [3, 2, 3, 2, 2];
const DOWN = { 2: [0.8, 1.3], 3: [1.25, 0.6, 1.05] } as const;

/**
 * Sweet spurge: a candelabra of thick grey succulent branches. A stem and five limbs leave the foot, each limb
 * forks into two or three twigs, and every twig ends on one dome in a rosette of narrow fleshy leaves, so the
 * rosettes make a round crown over branches that show beneath it. The summer drought sheds leaves: the crown
 * shrinks, the rosettes thin, and every fourth twig stands bare. 302 triangles, 240 in summer.
 */
function tabaiba(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(TABAIBA[season]);
    const [bark, barkLit] = [hex('#57554e'), hex('#9a978a')];
    const dry = season === 'summer';
    const k = dry ? 0.9 : 1;
    const cards = sheet();
    const parts: Geometry[] = [];
    const crown = (a: number, down: number) => v3(0.56 * k * Math.sin(down) * Math.cos(a), 0.56 * k * Math.sin(down) * Math.sin(a), 0.12 + 0.5 * k * Math.cos(down));
    const branch = (from: V, to: V, r: number, taper: number, sides: number) =>
        parts.push(paint(soft(tube([from.toArray(), to.toArray()], r, taper, 1, sides)), (p) => bark.clone().lerp(barkLit, smooth(0, 0.4, p.z))));
    let twigs = 0;
    // A twig from `from` to its rosette at `to`; a bare one tapers to a point a little short of the crown.
    const twig = (from: V, to: V, sides: number) => {
        const bare = dry && twigs++ % 4 === 1;
        const axis = to.clone().sub(from).normalize();
        branch(from, bare ? to.clone().addScaledVector(axis, -0.04) : to, sides === 4 ? 0.058 : 0.048, bare ? 0.15 : 0.7, sides);
        if (bare) return;
        axis.lerp(UP, 0.5).normalize();
        const [u, w] = across(axis), r = (dry ? 0.2 : 0.25) * (0.9 + 0.2 * rand()), spin = rand() * Math.PI;
        const sun = rand() < 0.4 ? 0.4 : 0.15 * rand();
        const leaves = dry ? 6 : 7;
        const light = (p: V) => p.clone().sub(to).divideScalar(r).addScaledVector(axis, 0.9).normalize().lerp(UP, 0.25).normalize();
        for (let j = 0; j < leaves; j++) {
            // The inner leaves stand, the outer ones spread, so the rosette is a cup with no hole.
            const t = spin + j * GOLDEN, tilt = 0.4 + 0.85 * ((j + 0.5) / leaves);
            const dir = axis.clone().multiplyScalar(Math.cos(tilt)).addScaledVector(u, Math.sin(tilt) * Math.cos(t)).addScaledVector(w, Math.sin(tilt) * Math.sin(t));
            const body = mid.clone().lerp(tip, sun + 0.25 * rand()).lerp(deep, 0.25 * smooth(0.7, 1.25, tilt));
            leaf(cards, to, dir, r * (0.85 + 0.3 * rand()), 0.4 * r, 0.55, [deep, body, body.clone().lerp(tip, 0.5)], light);
        }
    };
    twig(v3(0, 0, -0.02), crown(0, 0), 4);
    FORKS.forEach((count, i) => {
        const a = (i / FORKS.length) * 2 * Math.PI + 0.4 * rand();
        const fork = v3(0.2 * k * Math.cos(a), 0.2 * k * Math.sin(a), 0.2 * k);
        branch(v3(0.05 * Math.cos(a), 0.05 * Math.sin(a), -0.02), fork, 0.07, 0.8, 4);
        for (let j = 0; j < count; j++) twig(fork, crown(a + (j - (count - 1) / 2) * 0.62 + 0.2 * (rand() - 0.5), DOWN[count as 2 | 3][j]), 3);
    });
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Tabaiba',
    latin: 'Euphorbia balsamifera',
    build: (season, seed) => tabaiba(season, random(seed)),
    ground: { on: ['shrub', 'grass', 'bare'], height: [0.6, 1.2], share: 0.12, own: true },
} satisfies Plant;
