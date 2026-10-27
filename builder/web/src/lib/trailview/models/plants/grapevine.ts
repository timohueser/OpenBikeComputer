import type { Plant } from '../plant-library';
import { domed, hex, ramp, rampAt, random, sheet, v3, type Colour, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Vine leaves by season: fresh green, dark summer green, gold to rust in autumn, none in winter. */
const LEAVES: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#40622a', '#6e9a3c', '#a2c460'],
    summer: ['#2c4c20', '#4a752e', '#78a046'],
    autumn: ['#6a3c1c', '#b07a2c', '#d8a640'],
};
const WOOD = ['#3a3026', '#6e604c'].map(hex);

/** The piece's half-length, so a row of pieces 0.9 of their height apart overlaps a little. */
const HALF = 0.55;

/**
 * A piece of a trellised vine row, along x: two trunks to a cordon, and a hedge of leaves above it,
 * ragged at the top and lit as one rounded wall. Bare in winter: canes rise from the cordon to the
 * hedge's height, so the piece keeps its size. 45 triangles in leaf.
 */
function grapevine(season: Season, rand: () => number): Geometry {
    const cards = sheet();
    const [foot, bark] = WOOD;
    const strip = (a: V, b: V, width: number, colour: Colour) => {
        const side = v3().subVectors(b, a).cross(v3(0, 1, 0)).setLength(width / 2);
        cards.add([a.clone().sub(side), a.clone().add(side), b.clone().add(side)], [foot, foot, colour]);
        cards.add([a.clone().sub(side), b.clone().add(side), b.clone().sub(side)], [foot, colour, colour]);
    };
    for (const x of [-0.28 + 0.06 * rand(), 0.24 + 0.06 * rand()]) strip(v3(x, 0, 0), v3(x, 0, 0.42), 0.05, bark);
    strip(v3(-HALF, 0, 0.42), v3(HALF, 0, 0.42), 0.04, bark);
    const leaves = LEAVES[season];
    if (leaves === 'bare') {
        for (let k = 0; k < 7; k++) {
            const x = -HALF + (k + 0.5) * ((2 * HALF) / 7);
            const top = v3(x + 0.1 * (rand() - 0.5), 0.12 * (rand() - 0.5), k === 3 ? 1 : 0.7 + 0.28 * rand());
            cards.add([v3(x - 0.012, 0, 0.42), v3(x + 0.012, 0, 0.42), top], [bark, bark, bark]);
        }
        return cards.build();
    }
    const [deep, mid, tip] = ramp(leaves);
    const light = domed(v3(0, 0, 0.62), v3(1.2, 0.22, 0.4), 0.25);
    const shade = (z: number, tone: number) => rampAt([deep, mid, tip], 0.15 + 0.7 * (z - 0.45) / 0.55 + 0.25 * (tone - 0.5));
    // A dark core, so the hedge does not show through between its faces.
    cards.add([v3(-HALF, 0, 0.45), v3(HALF, 0, 0.45), v3(0, 0, 0.92)], [deep, deep, deep]);
    const columns = 4;
    for (const side of [-1, 1]) {
        for (let c = 0; c < columns; c++) {
            const [x0, x1] = [-HALF + (c * 2 * HALF) / columns - 0.04, -HALF + ((c + 1) * 2 * HALF) / columns + 0.04];
            const [y0, y1] = [side * (0.12 + 0.04 * rand()), side * (0.16 + 0.05 * rand())];
            const [a, b] = [v3(x0, y0, 0.42 + 0.08 * rand()), v3(x1, y0, 0.42 + 0.08 * rand())];
            const [d, e] = [v3(x0, y1, 0.78 + 0.08 * rand()), v3(x1, y1, 0.78 + 0.08 * rand())];
            const m = d.clone().lerp(e, 0.3 + 0.4 * rand());
            // Two shoot tips above each stretch of the hedge.
            const tip = (from: V, to: V, top: number) => from.clone().lerp(to, 0.3 + 0.4 * rand()).setY(side * 0.09).setZ(top);
            const tone = rand();
            const col = (p: V) => shade(p.z, tone);
            const faces = [[a, b, e], [a, e, d], [d, m, tip(d, m, c === 1 ? 1 : 0.88 + 0.1 * rand())], [m, e, tip(m, e, 0.86 + 0.12 * rand())]];
            for (const face of faces) cards.add(face, face.map(col), light);
        }
    }
    // Shoots across the top, where the hedge is trimmed least.
    for (let k = 0; k < 6; k++) {
        const x = -HALF + (k + 0.5) * ((2 * HALF) / 6);
        const s = k % 2 ? 1 : -1;
        const corners = [v3(x - 0.1, -s * 0.1, 0.84), v3(x + 0.1, -s * 0.06, 0.86), v3(x + 0.04 * (rand() - 0.5), s * 0.2, 0.95 + 0.05 * rand())];
        cards.add(corners, corners.map((p) => shade(p.z, rand())), light);
    }
    return cards.build();
}

export default {
    name: 'Grapevine',
    latin: 'Vitis vinifera',
    build: (season, seed) => grapevine(season, random(seed)),
    // One piece of a trellised row: it stands only along vineyard rows, never scattered, so share 0.
    ground: { on: ['crop'], height: [1.2, 1.7], share: 0 },
} satisfies Plant;
