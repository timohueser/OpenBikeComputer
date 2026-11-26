import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, kite, ramp, random, sheet, spindle, v3, type Geometry, type Ramp } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Reed stems and leaves: blue-green in summer, gold in autumn, straw in winter. */
const REED: Record<Season, Ramp> = {
    winter: ['#7a6440', '#b09868', '#d8c49a'],
    spring: ['#4a5e2a', '#78984a', '#a8c06e'],
    summer: ['#3a5630', '#62844a', '#94b070'],
    autumn: ['#7a6a30', '#a89848', '#d0bc6a'],
};

/** Plumes by season: the colour at the stem, the colour of the plume, and its size. */
const PLUMES: Record<Season, [string, string, number]> = {
    winter: ['#a89a84', '#e6ded0', 1.15],
    spring: ['#8a7a64', '#c8b89c', 0.85],
    summer: ['#5a4442', '#8e6a6c', 0.9],
    autumn: ['#8a7058', '#dccab0', 1.15],
};

/** Winter keeps a leaf on some stems, gone orange-brown. */
const WINTER_LEAF = hex('#a87444');

/**
 * A stand of common reed: 15 upright stems, each with four leaves that leave it at an angle and droop at the
 * tip, and a soft plume that nods at the top. Leaves and plumes point one way, as the wind combs a reed bed. In
 * spring 13 short green shoots stand among 7 of last year's stems.
 */
function commonReed(season: Season, rand: () => number): Geometry {
    const green = ramp(REED[season]);
    const straw = ramp(REED.winter);
    const [inner, edge, size] = PLUMES[season];
    const plume = [hex(inner), hex(edge)];
    const light = domed(v3(), v3(0.3, 0.3, 1), 0.4);
    const cards = sheet();
    const wind = rand() * 2 * Math.PI, lee = v3(Math.cos(wind), Math.sin(wind), 0);
    const stems = season === 'spring' ? 20 : 15;
    for (let k = 0; k < stems; k++) {
        const old = season === 'spring' && k % 3 === 0, shoot = season === 'spring' && !old, bare = old || season === 'winter';
        const [deep, mid, tip] = old ? straw : green;
        const a = k * GOLDEN + rand() * 0.5;
        const r = 0.22 * Math.sqrt((k + 0.5) / stems);
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const foot = out.clone().multiplyScalar(r);
        const length = (shoot ? 0.45 : 0.8) + 0.2 * rand();
        const lean = 0.03 + 0.3 * r + 0.05 * rand();
        const axis = out.clone().multiplyScalar(Math.sin(lean)).addScaledVector(lee, 0.06).setZ(Math.cos(lean));
        const top = foot.clone().addScaledVector(axis, length);
        // Two crossed blades, so a stem shows from every side; thicker than a real stem, as the stand has far fewer.
        for (const across of [v3(-out.y, out.x, 0), out]) {
            const thick = across.clone().multiplyScalar(0.012);
            cards.add([foot.clone().add(thick), foot.clone().sub(thick), top], [deep, deep, mid], light);
        }
        // Winter and last year's stems have shed their leaves, but for one here and there.
        const leaves = bare ? (old || k % 5 < 2 ? 1 : 0) : 4;
        for (let i = 0; i < leaves; i++) {
            const node = foot.clone().addScaledVector(axis, length * (0.25 + 0.17 * i + 0.06 * rand()));
            const b = wind + (rand() - 0.5) * 2.2;
            const dir = v3(Math.cos(b), Math.sin(b), 0);
            const [l, tilt] = [(shoot ? 0.2 : 0.24) + 0.08 * rand(), 0.6 + 0.4 * rand()];
            const bend = node.clone().addScaledVector(dir, 0.45 * l * Math.sin(tilt)).setZ(node.z + 0.45 * l * Math.cos(tilt));
            const end = node.clone().addScaledVector(dir, 1.1 * l * Math.sin(tilt)).setZ(node.z + l * (Math.cos(tilt) - 0.45));
            const body = bare ? WINTER_LEAF : mid.clone().lerp(tip, 0.3 * rand());
            kite(cards, node, bend, end, v3(-dir.y, dir.x, 0).multiplyScalar(0.024 * (0.8 + 0.4 * rand())), [deep.clone().lerp(body, 0.5), body, bare ? body.clone().lerp(tip, 0.3) : tip], light);
        }
        if (shoot) continue;
        // The plume nods down the wind.
        const nod = 0.5 + 0.5 * rand();
        const hang = axis.clone().multiplyScalar(Math.cos(nod)).addScaledVector(lee, Math.sin(nod)).normalize();
        spindle(cards, top, top.clone().addScaledVector(hang, 0.2 * size * (0.85 + 0.3 * rand())), 0.036 * size, 4, plume);
    }
    return cards.build();
}

export default {
    name: 'Common reed',
    latin: 'Phragmites australis',
    build: (season, seed) => commonReed(season, random(seed)),
    ground: { on: ['wetland'], height: [1.5, 3], share: 0.5, own: true, clumped: true },
} satisfies Plant;
