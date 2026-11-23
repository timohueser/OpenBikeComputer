import type { Plant } from '../plant-library';
import { computeNormals, hex, merge, paint, ramp, random, sheet, spike, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The pads: grey-green, the same all year. */
const PAD: Ramp = ['#43573c', '#6e8662', '#9cb08a'];

/** Yellow flowers on the rims in late spring, red-purple fruit from late summer: colour and count. */
const RIM: Record<Season, [string, number]> = {
    winter: ['#7a3048', 0],
    spring: ['#f0c834', 9],
    summer: ['#a8284a', 9],
    autumn: ['#8a2a48', 6],
};

/** The pads that stand on the ground, the pads that each pad of a level carries, and the corners of a pad's rim. */
const BASE = 3, CARRIES = [[2, 2, 2], [2, 2, 1, 1, 1, 1]], SIDES = 8;

/**
 * Prickly pear: a clump of thick oval pads, three from the ground and each carrying one or two more on its upper
 * rim, three deep, every pad turned its own way. A pad is a smooth lens, wider toward its top and paler in its
 * middle; flowers or fruit sit as small knobs on the upper rims of the top pads.
 */
function pricklyPear(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(PAD);
    const [fruit, fruits] = RIM[season];
    const knob = hex(fruit);
    const cards = sheet();
    const parts: Geometry[] = [];
    const tops: { rim: V[]; centre: V }[] = [];
    const pad = (centre: V, up: V, side: V, length: number, level: number, nth: number) => {
        const tone = 0.88 + 0.24 * rand();
        const face = v3().crossVectors(up, side).multiplyScalar(0.11 * length);
        const rim = Array.from({ length: SIDES }, (_, i) => {
            const t = (i / SIDES) * 2 * Math.PI;
            return centre.clone().addScaledVector(up, 0.5 * length * Math.cos(t)).addScaledVector(side, (0.34 + 0.07 * Math.cos(t)) * length * Math.sin(t));
        });
        const index: number[] = [];
        for (let i = 0; i < SIDES; i++) index.push(SIDES, i, (i + 1) % SIDES, SIDES + 1, (i + 1) % SIDES, i);
        const lens = computeNormals({ position: [...rim, centre.clone().add(face), centre.clone().sub(face)].flatMap((p) => p.toArray()), normal: [], color: [], index });
        // Young pads at the top are a fresher green.
        parts.push(paint(lens, (p) => (p.distanceTo(centre) < 0.2 * length ? mid.clone().lerp(tip, 0.3 + 0.2 * level) : deep.clone().lerp(mid, 0.45 + 0.15 * level)).multiplyScalar(tone)));
        if (level === 2) tops.push({ rim, centre });
        for (let c = 0; c < (CARRIES[level]?.[nth] ?? 0); c++) {
            // A child grows from the upper rim, leaning out and turning its face.
            const phi = (c ? -1 : 1) * (0.25 + 0.5 * rand());
            const at = centre.clone().addScaledVector(up, 0.46 * length * Math.cos(phi)).addScaledVector(side, 0.32 * length * Math.sin(phi));
            const dir = up.clone().applyAxisAngle(v3().crossVectors(up, side), -phi * 0.9).add(v3(0, 0, 0.3)).normalize();
            const turned = side.clone().projectOnPlane(dir).normalize().applyAxisAngle(dir, (rand() - 0.5) * 2.2);
            const size = length * (0.82 + 0.2 * rand());
            pad(at.clone().addScaledVector(dir, 0.44 * size), dir, turned, size, level + 1, level ? 0 : 2 * nth + c);
        }
    };
    for (let k = 0; k < BASE; k++) {
        const a = (k / BASE) * 2 * Math.PI + rand();
        // The first pad stands; the others lean out, so the clump has a middle.
        const lean = k ? 0.5 : 0.12, up = v3(lean * Math.cos(a), lean * Math.sin(a), 1).normalize();
        const side = v3(-Math.sin(a + rand()), Math.cos(a + rand()), 0).projectOnPlane(up).normalize();
        pad(v3(0.1 * Math.cos(a), 0.1 * Math.sin(a), 0.17), up, side, 0.4, 0, k);
    }
    // A knob stands on a corner of a top pad's upper rim, pointing out of the pad.
    for (let k = 0; k < fruits; k++) {
        const { rim, centre } = tops[k % tops.length], at = rim[[0, 1, SIDES - 1][Math.floor(k / tops.length) % 3]];
        const out = at.clone().sub(centre).normalize(), size = 0.04 + 0.01 * rand();
        spike(cards, at.clone().addScaledVector(out, -0.4 * size), at.clone().addScaledVector(out, 1.3 * size), size, [knob.clone().multiplyScalar(0.7), knob]);
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Prickly pear',
    latin: 'Opuntia ficus-indica',
    build: (season, seed) => pricklyPear(season, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1.5, 3], share: 0.02, own: true, clumped: true },
} satisfies Plant;
