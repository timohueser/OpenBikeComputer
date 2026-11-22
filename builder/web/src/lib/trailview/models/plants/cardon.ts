import type { Plant } from '../plant-library';
import { computeNormals, hex, merge, paint, ramp, random, smooth, UP, v3, type Colour, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Cardón stems: glaucous grey-green after the winter rains, yellower in the drought. */
const CARDON: Record<Season, Ramp> = {
    winter: ['#374e3b', '#62845e', '#a0b488'],
    spring: ['#38503c', '#668862', '#a6b88a'],
    summer: ['#465040', '#74825a', '#aaac7c'],
    autumn: ['#3a4f3c', '#68845e', '#a2b286'],
};

/** Red cyathia round the stem tips in spring, red-brown capsules in summer. */
const CARDON_FRUIT: Partial<Record<Season, string>> = { spring: '#c2443a', summer: '#96503a' };

/** Sides of a stem, the stems of the middle bundle and of the ring round it, and the fruits of a stem in flower. */
const RIBS = 5, INNER = 4, OUTER = 5, FRUITS = 5;

/**
 * A five-ribbed stem through `path`, `r` thick: narrower at its foot, and closed above its last
 * point by a dome of one smaller ring and a tip. Its normals are smooth, so it lights as a round
 * column. Returns the stem and the ring of its dome, where its fruits sit. 10 triangles a
 * segment, 15 for the dome.
 */
function stem(path: V[], r: number, spin: number, shade: (p: V) => Colour): { geo: Geometry; top: V[] } {
    const end = path[path.length - 1], up = end.clone().sub(path[path.length - 2]).normalize();
    const spine = [...path, end.clone().addScaledVector(up, 0.55 * r)], last = spine.length - 1;
    const radius = (i: number) => (i === 0 ? 0.8 : i === last ? 0.6 : 1) * r;
    const dirs = spine.slice(1).map((p, i) => p.clone().sub(spine[i]).normalize());
    // Each ring's frame comes down from the one above it, so a bent stem does not twist.
    let u = v3(Math.cos(spin), Math.sin(spin), 0);
    const rings: V[][] = [];
    for (let i = last; i >= 0; i--) {
        const [a, b] = [dirs[Math.max(i - 1, 0)], dirs[Math.min(i, last - 1)]];
        const t = a.clone().add(b).normalize();
        u = u.projectOnPlane(t).normalize();
        const w = v3().crossVectors(t, u);
        // A ring at a bend lies in the mitre plane, stretched across the bend to keep the stem's girth.
        const bend = b.clone().sub(a).projectOnPlane(t);
        const stretch = 1 / t.dot(b) - 1;
        if (bend.lengthSq() > 1e-6) bend.normalize();
        rings[i] = Array.from({ length: RIBS }, (_, j) => {
            const off = u.clone().multiplyScalar(radius(i) * Math.cos((j * 2 * Math.PI) / RIBS)).addScaledVector(w, radius(i) * Math.sin((j * 2 * Math.PI) / RIBS));
            return spine[i].clone().add(off).addScaledVector(bend, stretch * off.dot(bend));
        });
    }
    const corners = [...rings.flat(), spine[last].clone().addScaledVector(up, 0.3 * r)];
    const index: number[] = [];
    for (let i = 0; i <= last; i++) for (let j = 0; j < RIBS; j++) {
        const [a, b] = [RIBS * i + j, RIBS * i + ((j + 1) % RIBS)];
        if (i < last) index.push(a, b, b + RIBS, a, b + RIBS, a + RIBS);
        else index.push(a, b, corners.length - 1);
    }
    return { geo: paint(computeNormals({ position: corners.flatMap(p => p.toArray()), normal: [], color: [], index }), shade), top: rings[last] };
}

/** A fruit at `at`: a small round knob that points `out` of the stem, its foot in the stem. 3 triangles. */
function fruit(at: V, out: V, size: number, colour: Colour): Geometry {
    const u = v3().crossVectors(out, UP).normalize(), w = v3().crossVectors(out, u);
    const foot = [0, 1, 2].map(k => at.clone().addScaledVector(out, -0.3 * size).addScaledVector(u, size * Math.cos((k * 2 * Math.PI) / 3)).addScaledVector(w, size * Math.sin((k * 2 * Math.PI) / 3)));
    const corners = [...foot, at.clone().addScaledVector(out, size)];
    return paint(computeNormals({ position: corners.flatMap(p => p.toArray()), normal: [], color: [], index: [0, 1, 3, 1, 2, 3, 2, 0, 3] }), colour);
}

/**
 * Cardón: a candelabra of nine thick five-ribbed stems with domed tips. Four stand in a tight
 * bundle in the middle; five round them leave the base sideways and bend upright at an elbow.
 * In spring and summer the middle stem and one outer stem carry a crown of fruits round the tip, one on each rib.
 */
function cardon(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(CARDON[season]);
    const [wood, bloom] = [hex('#6a6450'), CARDON_FRUIT[season]];
    const parts: Geometry[] = [];
    const add = (path: V[], r: number, flowers: boolean) => {
        const end = path[path.length - 1], h = end.z + 0.85 * r;
        const tone = 0.9 + 0.2 * rand();
        const shade = (p: V) => deep.clone().lerp(mid, smooth(0, 0.4, p.z)).lerp(tip, 0.8 * smooth(h - 0.35, h, p.z)).multiplyScalar(tone).lerp(wood, 0.6 * (1 - smooth(0, 0.12, p.z)));
        const { geo, top } = stem(path, r, rand() * Math.PI, shade);
        parts.push(geo);
        if (bloom && flowers) for (const at of top.slice(0, FRUITS)) parts.push(fruit(at, at.clone().sub(end).setZ(0).normalize().setZ(0.8).normalize(), 0.035, hex(bloom)));
    };
    // A tight bundle in the middle, its stems touching: one at the centre and three around it.
    for (let k = 0; k < INNER; k++) {
        const a = (k / (INNER - 1)) * 2 * Math.PI + rand() * 0.3;
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const d = k ? 0.2 + 0.03 * rand() : 0.02;
        const r = 0.115 * (0.92 + 0.16 * rand());
        add([out.clone().multiplyScalar(0.7 * d).setZ(-0.03), out.clone().multiplyScalar(d + 0.03).setZ(0.88 - 0.5 * d - 0.12 * rand())], r, k === 0);
    }
    for (let k = 0; k < OUTER; k++) {
        const a = ((k + 0.5) / OUTER) * 2 * Math.PI + (rand() - 0.5) * 0.3;
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const d = 0.4 + 0.08 * rand();
        const r = 0.115 * (0.92 + 0.16 * rand());
        add([out.clone().multiplyScalar(0.14).setZ(0), out.clone().multiplyScalar(d).setZ(0.2 + 0.05 * rand()), out.clone().multiplyScalar(d + 0.04).setZ(0.42 + 0.22 * rand())], r, k === 2);
    }
    return merge(parts);
}

export default {
    name: 'Cardón',
    latin: 'Euphorbia canariensis',
    build: (season, seed) => cardon(season, random(seed)),
    ground: { on: ['shrub', 'bare'], height: [1, 2], share: 0.03, own: true },
} satisfies Plant;
