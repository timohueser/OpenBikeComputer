import type { Plant } from '../plant-library';
import { hex, ramp, random, sheet, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The pads: grey-green, the same all year. */
const PAD: Ramp = ['#43573c', '#6e8662', '#9cb08a'];

/** Yellow flowers on the rims in late spring, red-purple fruit from late summer: colour and share of rim places. */
const RIM: Record<Season, [string, number]> = {
    winter: ['#7a3048', 0],
    spring: ['#f0c834', 0.5],
    summer: ['#a8284a', 0.6],
    autumn: ['#8a2a48', 0.3],
};

/**
 * Prickly pear: a clump of flat oval pads, four from the ground and each carrying one or two more
 * on its upper rim, four deep, every pad turned its own way. A pad is a flat fan of eight
 * triangles, paler at its middle; flowers or fruit sit as small diamonds on the top pads' rims.
 * About 300 triangles with fruit.
 */
function pricklyPear(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(PAD);
    const [fruit, share] = RIM[season];
    const knob = hex(fruit);
    const cards = sheet();
    const pad = (centre: V, up: V, across: V, length: number, depth: number) => {
        const tone = 0.85 + 0.3 * rand();
        const middle = mid.clone().lerp(tip, 0.4).multiplyScalar(tone);
        const edge = deep.clone().lerp(mid, 0.5).multiplyScalar(tone);
        const rim = Array.from({ length: 8 }, (_, i) => {
            const t = (i / 8) * 2 * Math.PI;
            return centre.clone().addScaledVector(up, 0.5 * length * Math.cos(t)).addScaledVector(across, 0.38 * length * Math.sin(t));
        });
        for (let i = 0; i < 8; i++) cards.add([centre, rim[i], rim[(i + 1) % 8]], [middle, edge, edge]);
        // Draws for every pad in every season, so the clump keeps its shape the year round.
        const [roll, count, children] = [rand(), 1 + Math.floor(rand() * 3), depth < 3 ? 1 + Math.round(rand()) : 0];
        if (!children && roll < share) {
            for (const i of [1, 2, 3].slice(0, count)) {
                const at = rim[i % 8];
                const s = 0.08 * length;
                cards.add([at.clone().add(v3(0, 0, 2 * s)), at.clone().addScaledVector(across, s), at.clone().addScaledVector(across, -s)], [knob, knob.clone().multiplyScalar(0.7), knob.clone().multiplyScalar(0.7)]);
            }
        }
        for (let c = 0; c < children; c++) {
            // A child grows from the upper rim, leaning out and turning its face.
            const phi = (rand() - 0.5) * 1.6;
            const at = centre.clone().addScaledVector(up, 0.48 * length * Math.cos(phi)).addScaledVector(across, 0.32 * length * Math.sin(phi));
            const normal = v3().crossVectors(up, across);
            const dir = up.clone().applyAxisAngle(normal, -phi * 0.8).add(v3(0, 0, 0.35)).normalize();
            const turn = (rand() - 0.5) * 2.4;
            const side = across.clone().projectOnPlane(dir).normalize().applyAxisAngle(dir, turn);
            const size = length * (0.85 + 0.2 * rand());
            pad(at.clone().addScaledVector(dir, 0.45 * size), dir, side, size, depth + 1);
        }
    };
    for (let k = 0; k < 4; k++) {
        const a = (k / 4) * 2 * Math.PI + rand();
        const up = v3(0.35 * Math.cos(a), 0.35 * Math.sin(a), 1).normalize();
        const across = v3(-Math.sin(a + rand()), Math.cos(a + rand()), 0).projectOnPlane(up).normalize();
        pad(v3(0.12 * Math.cos(a), 0.12 * Math.sin(a), 0.17), up, across, 0.36, 0);
    }
    return cards.build();
}

export default {
    name: 'Prickly pear',
    latin: 'Opuntia ficus-indica',
    build: (season, seed) => pricklyPear(season, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1.5, 3], share: 0.02, own: true, clumped: true },
} satisfies Plant;
