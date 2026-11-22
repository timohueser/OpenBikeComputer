import type { Plant } from '../plant-library';
import { computeNormals, hex, leaf, merge, paint, ramp, rampAt, random, sheet, spike, UP, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Vine leaves by season: fresh green, dark summer green, gold to rust in autumn, none in winter. */
const LEAVES: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#40622a', '#6e9a3c', '#a2c460'],
    summer: ['#2c4c20', '#4a752e', '#78a046'],
    autumn: ['#6a3c1c', '#b07a2c', '#d8a640'],
};
const WOOD = ['#3a3026', '#6e604c'].map(hex);

/** Ripe bunches hang under the hedge in autumn. */
const GRAPE = ['#3a2a4e', '#5a3c6e'].map(hex);

/** The piece's half-length, so a row of pieces 0.9 of their height apart overlaps a little. */
const HALF = 0.55;

/** The hedge's section across the row, (y, z), from one foot over the top to the other. */
const SECTION: [number, number][] = [[-0.13, 0.44], [-0.21, 0.66], [0, 0.88], [0.21, 0.66], [0.13, 0.44]];

/**
 * A piece of a trellised vine row, along x: two trunks to a cordon and, above it, a rounded hedge of leaves with
 * shoots that stand out of its top; in autumn two bunches of grapes hang under it. Bare in winter: canes rise from
 * the cordon to the hedge's height, so the piece keeps its size.
 */
function grapevine(season: Season, rand: () => number): Geometry {
    const cards = sheet();
    const [foot, bark] = WOOD;
    for (const x of [-0.28 + 0.06 * rand(), 0.24 + 0.06 * rand()]) spike(cards, v3(x, 0, 0), v3(x + 0.06 * (rand() - 0.5), 0, 0.56), 0.035, [foot, bark]);
    const leaves = LEAVES[season];
    if (leaves === 'bare') {
        const cordon = [v3(-HALF, 0, 0.4), v3(-HALF, 0, 0.45), v3(HALF, 0, 0.45), v3(HALF, 0, 0.4)];
        cards.add([cordon[0], cordon[3], cordon[2]], [bark, bark, bark]);
        cards.add([cordon[0], cordon[2], cordon[1]], [bark, bark, bark]);
        for (let k = 0; k < 5; k++) {
            const x = -HALF + (k + 0.5) * ((2 * HALF) / 5);
            const top = v3(x + 0.1 * (rand() - 0.5), 0.12 * (rand() - 0.5), k === 2 ? 1 : 0.7 + 0.28 * rand());
            cards.add([v3(x - 0.015, 0, 0.43), v3(x + 0.015, 0, 0.43), top], [bark, bark, bark]);
        }
        return cards.build();
    }
    const [deep, mid, tip] = ramp(leaves);
    const shade = (p: V) => rampAt([deep, mid, tip], 0.1 + 0.75 * (p.z - 0.44) / 0.5 + 0.2 * (rand() - 0.5));
    // The hedge: its section at both ends and, fuller, in the middle, closed at the ends.
    const rings = [-HALF, 0, HALF].map((x, i) => SECTION.map(([y, z]) => v3(x + 0.06 * (rand() - 0.5), y * (i === 1 ? 1.2 : 0.75) * (0.9 + 0.2 * rand()), 0.66 + (z - 0.66) * (i === 1 ? 1.05 : 0.8) + 0.04 * (rand() - 0.5))));
    const index: number[] = [];
    for (let i = 0; i < 2; i++) for (let j = 0; j < 4; j++) {
        const [a, b] = [5 * i + j, 5 * i + j + 5];
        index.push(a, b + 1, a + 1, a, b, b + 1);
    }
    for (let j = 1; j < 4; j++) index.push(0, j, j + 1, 10, 11 + j, 10 + j);
    const hedge = paint(computeNormals({ position: rings.flat().flatMap((p) => p.toArray()), normal: [], color: [], index }), shade);
    const grapes = season === 'autumn' ? 2 : 0, shoots = grapes ? 5 : 8;
    for (let k = 0; k < shoots; k++) {
        const side = k % 2 ? 1 : -1, x = -HALF + (k + 0.5) * ((2 * HALF) / shoots);
        const at = v3(x, side * 0.08, 0.76), dir = v3(0.5 * (rand() - 0.5), side * (0.25 + 0.4 * rand()), 1).normalize();
        const body = rampAt([deep, mid, tip], 0.6 + 0.3 * rand());
        leaf(cards, at, dir, k === 2 ? 0.27 : 0.16 + 0.1 * rand(), 0.17, 0.5, [mid, body, tip], () => dir.clone().lerp(UP, 0.5).normalize());
    }
    for (let k = 0; k < grapes; k++) {
        const at = v3((k ? 0.22 : -0.2) + 0.1 * rand(), k ? 0.17 : -0.17, 0.54);
        spike(cards, at, at.clone().setZ(0.32), 0.06, [GRAPE[1], GRAPE[0]]);
    }
    return merge([hedge, cards.build()]);
}

export default {
    name: 'Grapevine',
    latin: 'Vitis vinifera',
    build: (season, seed) => grapevine(season, random(seed)),
    // One piece of a trellised row: it stands only along vineyard rows, never scattered, so share 0.
    ground: { on: ['crop'], height: [1.2, 1.7], share: 0 },
} satisfies Plant;
