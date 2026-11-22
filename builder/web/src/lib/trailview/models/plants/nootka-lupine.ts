import type { Plant } from '../plant-library';
import { across, domed, leaf, ramp, random, sheet, spike, spindle, UP, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** The seasons in which the clump stands; it dies back in winter. */
type Living = Exclude<Season, 'winter'>;

/** The palmate leaves: mid green. */
const LEAVES: Record<Living, Ramp> = {
    spring: ['#355a2c', '#527a40', '#7ca062'],
    summer: ['#33562a', '#4e763e', '#76985c'],
    autumn: ['#4a5a2c', '#6e7a3e', '#94985a'],
};

/** The spikes from foot to tip: blue-purple in flower in early summer, green pods under the last flowers after, then dry brown. */
const SPIKES: Record<Living, Ramp> = {
    spring: ['#3e3088', '#5a4cb0', '#b0a6e6'],
    summer: ['#4a6a30', '#62708a', '#6a5cc0'],
    autumn: ['#5a4630', '#7a6040', '#9a7c54'],
};

/**
 * Nootka lupine: a bushy clump of seven palmate leaves, each five leaflets cupped round the end of
 * its stalk, and five dense flower spikes that stand above it. A spike is a spindle on its stalk,
 * widest near its foot and drawn out to a point.
 */
function lupine(season: Living, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LEAVES[season]);
    const cards = sheet();
    const bush = domed(v3(0, 0, 0.1), v3(0.5, 0.5, 0.6), 0.5);
    for (let k = 0; k < 7; k++) {
        const a = (k / 7) * 2 * Math.PI + rand() * 0.6;
        const d = 0.2 + 0.2 * rand();
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const at = out.clone().multiplyScalar(d).setZ(0.24 + 0.28 * rand());
        spike(cards, v3(), at, 0.016, [deep, mid]);
        const [u, w] = across(out.clone().multiplyScalar(0.5).add(UP).normalize()), face = v3().crossVectors(u, w), turn = rand() * 6;
        for (let j = 0; j < 5; j++) {
            const b = turn + (j / 5) * 2 * Math.PI;
            const dir = u.clone().multiplyScalar(Math.cos(b)).addScaledVector(w, Math.sin(b)).addScaledVector(face, 0.3).normalize();
            leaf(cards, at, dir, 0.19 + 0.03 * rand(), 0.1, 0.62, [deep.clone().lerp(mid, 0.6), mid, tip], bush);
        }
    }
    const colours = ramp(SPIKES[season]);
    for (let k = 0; k < 5; k++) {
        const a = (k / 5) * 2 * Math.PI + rand();
        const d = 0.04 + 0.17 * rand();
        const foot = v3(d * Math.cos(a), d * Math.sin(a), 0.42 + 0.14 * rand());
        const top = foot.clone().add(v3(0.04 * Math.cos(a), 0.04 * Math.sin(a), 0.46 + 0.16 * rand()));
        spike(cards, v3(), foot.clone().lerp(top, 0.1), 0.02, [deep, mid]);
        spindle(cards, foot, top, 0.058, 4, colours, 0.24);
    }
    return cards.build();
}

export default {
    name: 'Nootka lupine',
    latin: 'Lupinus nootkatensis',
    build: (season, seed) => (season === 'winter' ? undefined : lupine(season, random(seed))),
    ground: { on: ['bare', 'grass', 'shrub'], height: [0.5, 1], share: 0.3, own: true, clumped: true },
} satisfies Plant;
