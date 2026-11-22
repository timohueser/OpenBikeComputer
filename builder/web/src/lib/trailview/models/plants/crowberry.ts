import type { Plant } from '../plant-library';
import { cushion, domed, hex, leaf, merge, onDome, paint, place, ramp, random, sheet, smooth, soft, tetrahedron, UP, v3, type Geometry, type Ramp, type Tuft, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Crowberry needles: dark green with fresh yellow-green shoot tips, bronzed olive-brown in winter. */
const CROWBERRY: Record<Season, Ramp> = {
    winter: ['#36301e', '#5e5434', '#86744a'],
    spring: ['#24361e', '#46663a', '#7a9048'],
    summer: ['#223a20', '#426c3a', '#8eac52'],
    autumn: ['#263620', '#465e36', '#7e8044'],
};

/** Share of shoots with fresh growth at the tip, and the black berries on the mat, by season. */
const FRESH: Record<Season, number> = { winter: 0, spring: 0.25, summer: 0.6, autumn: 0.15 };
const BERRIES: Record<Season, number> = { winter: 5, spring: 3, summer: 9, autumn: 11 };

const STEM = hex('#6a3424');
const BERRY = hex('#141418');
const SHINE = hex('#4a4e60');

/**
 * Crowberry: a low mat about four times as wide as tall, three broad cushions grown together and two small ones at
 * their rim. Short needle shoots stand close on the cushions, the fresh ones yellow-green; red-brown stems creep
 * out from under the rim and turn up into a shoot, and glossy black berries sit on the cushions' tops. 186
 * triangles and 4 for each berry.
 */
function crowberry(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(CROWBERRY[season]);
    const colours = [deep.clone().multiplyScalar(0.8), deep.clone().lerp(mid, 0.5), mid];
    const cards = sheet();
    const turn = rand() * 2 * Math.PI;
    const mat = (k: number, of: number, d: number, r: number, h: number): Tuft => {
        const a = turn + ((k + 0.4 * rand()) / of) * 2 * Math.PI;
        return [d * Math.cos(a), d * Math.sin(a), r * (1 + 0.2 * rand()), h * (1 + 0.2 * rand())];
    };
    const broad = [0, 1, 2].map((k) => mat(k, 3, 0.7, 0.85, 0.62));
    const small = [0, 1].map((k) => mat(3 * k + 1, 6, 1.4, 0.5, 0.35));
    const parts = [...broad.map((m, k) => cushion(m, 8, colours, 0.4, seed + k)), ...small.map((m, k) => cushion(m, 6, colours, 0.4, seed + 3 + k))];
    const needles = (low: number) => mid.clone().lerp(tip, rand() < FRESH[season] * (1 - low) ? 0.7 + 0.3 * rand() : 0.3 * rand());
    const spots: V[] = [];
    broad.forEach((m, k) => {
        const [x, y, r, h] = m, light = domed(v3(x, y, 0), v3(r, r, h), 0.4), shoots = k ? 9 : 11, spin = rand() * 2 * Math.PI;
        for (let j = 0; j < shoots; j++) {
            const [foot, out] = onDome(m, j, shoots, 1.3, spin);
            const body = needles(0);
            leaf(cards, foot, out.clone().multiplyScalar(0.7).add(UP).normalize(), 0.34 + 0.12 * rand(), 0.24, 0.5, [colours[1], body, body.clone().lerp(tip, 0.4)], light);
        }
        for (let j = 0; j < 4; j++) spots.push(onDome(m, j, 4, 0.75, spin + 1.2)[0]);
    });
    // Red-brown stems creep out from under the rim and turn up into a shoot.
    const rim = domed(v3(), v3(1.7, 1.7, 0.6), 0.5);
    for (let j = 0; j < 10; j++) {
        const a = ((j + rand() * 0.7) / 10) * 2 * Math.PI, out = v3(Math.cos(a), Math.sin(a), 0.12).normalize();
        leaf(cards, out.clone().multiplyScalar(1.1).setZ(0.02), out, 0.65 + 0.25 * rand(), 0.22, 0.7, [STEM, STEM.clone().lerp(mid, 0.6), needles(0.3)], rim);
    }
    // Berries sit on the cushions' tops, half in the needles.
    for (let b = 0; b < BERRIES[season]; b++) {
        const at = spots.splice(Math.floor(rand() * spots.length), 1)[0];
        const ball = place(soft(tetrahedron(0.15)), { at: [at.x, at.y, at.z + 0.09], rot: [0, 0, rand() * Math.PI] });
        parts.push(paint(ball, (p) => BERRY.clone().lerp(SHINE, smooth(at.z + 0.05, at.z + 0.22, p.z))));
    }
    return merge([...parts, cards.build()]);
}

export default {
    name: 'Crowberry',
    latin: 'Empetrum nigrum subsp. hermaphroditum',
    build: (season, seed) => crowberry(season, seed, random(seed)),
    ground: { on: ['#moor', 'moss'], height: [0.1, 0.25], share: 0.3, own: true },
} satisfies Plant;
