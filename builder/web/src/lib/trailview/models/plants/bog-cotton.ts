import type { Plant } from '../plant-library';
import { blade, GOLDEN, hex, lump, merge, octahedron, paint, place, ramp, random, ribbon, smooth, soft, turnFromTo, UP, v3, type Colour, type Geometry, type Ramp, type V, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Cotton-grass leaves: green with russet tips, crimson in autumn, straw in winter. */
const SEDGE: Record<Season, Ramp> = {
    winter: ['#56442e', '#86704a', '#ac966a'],
    spring: ['#3c5a2a', '#66883c', '#a2a456'],
    summer: ['#3a5828', '#62823a', '#a08a4a'],
    autumn: ['#52241a', '#8a3424', '#bc5a34'],
};

/**
 * A head on a stalk tip, its foot on the tip and its length along `axis`: a soft ball with its
 * corners pushed out a little.
 */
function head(ball: Geometry, tip: V, axis: V, stretch: number, low: Colour, high: Colour, rand: () => number) {
    const geo = soft(ball);
    const p = v3(), count = geo.position.length / 3;
    for (let i = 0; i < count; i++) geo.position.splice(3 * i, 3, ...p.from(geo.position, i).multiplyScalar(1 + 0.15 * rand()).toArray());
    // Normals lean up, so the head lights as one soft tuft rather than a cut stone.
    for (let i = 0; i < count; i++) geo.normal.splice(3 * i, 3, ...p.from(geo.position, i).normalize().lerp(v3(0, 0, 1), 0.5).normalize().toArray());
    place(geo, { rot: [0, 0, rand() * 2 * Math.PI] });
    place(geo, { scale: [1, 1, stretch] });
    const zs = geo.position.filter((_, i) => i % 3 === 2), min = { z: Math.min(...zs) }, max = { z: Math.max(...zs) };
    place(geo, { at: tip.clone().addScaledVector(axis, -0.85 * min.z), turn: turnFromTo(UP, axis) });
    return paint(geo, (q) => low.clone().lerp(high, smooth(tip.z + 0.3 * min.z, tip.z + 0.8 * max.z, q.z)));
}

/**
 * A cotton-grass tuft: seven narrow arching leaves and seven thin stalks, each topped in summer by
 * a white cotton tuft that nods off it and in spring by a dark flower spikelet. Out of flower the
 * stalks taper, lean and break like dry stems. 175 triangles in summer, 91 in spring, 42 without
 * heads.
 */
function bogCotton(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(SEDGE[season]);
    const leaf = (p: V) => deep.clone().lerp(mid, smooth(0, 0.2, p.length())).lerp(tip, smooth(0.25, 0.55, p.length()));
    const dead = season === 'autumn' || season === 'winter';
    const parts: Geometry[] = [];
    for (let k = 0; k < 7; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        parts.push(paint(blade([0.03 * Math.cos(a), 0.03 * Math.sin(a), 0], a, 0.45 + 0.2 * rand(), 0.25 + 0.5 * rand(), dead ? 0.6 : 0.4, (t) => 0.07 * (1 - t), 2), leaf));
    }
    const stalk = hex(dead ? '#8a6e48' : '#6e7a48');
    for (let k = 0; k < 7; k++) {
        const a = k * GOLDEN + 1 + rand() * 0.6;
        const [c, s] = [Math.cos(a), Math.sin(a)];
        const d = 0.08 + 0.22 * rand();
        const foot: Vec = [0.02 * c, 0.02 * s, 0];
        const top = v3(d * c, d * s, 0.75 + 0.25 * rand());
        if (!dead) {
            parts.push(paint(ribbon([foot, top.toArray()], [-s, c, 0], (t) => 0.016 * (1 - 0.4 * t), 1), stalk));
            const up = top.clone().sub(v3(...foot)).normalize();
            if (season === 'spring') {
                parts.push(head(octahedron(0.026), top, up, 1.5, hex('#3e3226'), hex('#6a5a40'), rand));
                continue;
            }
            // The cotton nods out, away from the tuft.
            const tilt = 0.5 + 0.5 * rand();
            const nod = up.clone().multiplyScalar(Math.cos(tilt)).add(v3(c, s, 0).multiplyScalar(Math.sin(tilt))).normalize();
            parts.push(head(lump(0.04, 0), top, nod, 1.4, hex('#e2ded2'), hex('#ffffff'), rand));
            continue;
        }
        // Dry stems: one in three broken over at the kink, one in three shorter.
        const broken = k % 3 === 1;
        if (k % 3 === 2) top.z *= 0.7;
        // At half the stem, where the ribbon puts its middle vertex.
        const kink = v3(...foot).lerp(top, 0.5);
        const bend = broken ? 0.7 + 0.6 * rand() : 0.15 * rand();
        const rest = top.distanceTo(kink);
        const over = v3(c, s, 0).multiplyScalar(Math.sin(bend)).add(top.clone().sub(kink).normalize().multiplyScalar(Math.cos(bend))).normalize();
        const end = kink.clone().addScaledVector(over, rest);
        parts.push(paint(ribbon([foot, kink.toArray(), end.toArray()], [-s, c, 0], (t) => 0.018 * (1 - t), 2), stalk));
    }
    return merge(parts);
}

export default {
    name: 'Bog cotton',
    latin: 'Eriophorum angustifolium',
    build: (season, seed) => bogCotton(season, random(seed)),
    ground: { on: ['wetland', 'moss', 'grass'], height: [0.3, 0.5], share: 0.08, habitat: 'moor', clumped: true },
} satisfies Plant;
