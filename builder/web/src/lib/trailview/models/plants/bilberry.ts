import type { Plant } from '../plant-library';
import { domed, GOLDEN, hex, merge, octahedron, paint, place, ramp, random, sheet, smooth, soft, turnFromTo, UP, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Bilberry leaves: bright green, flame red to crimson in autumn, shed in winter. */
const BILBERRY: Record<Season, Ramp | 'bare'> = {
    winter: 'bare',
    spring: ['#3e7a2a', '#6eaa3a', '#acd65c'],
    summer: ['#356a28', '#5a9634', '#90c04c'],
    autumn: ['#6e1020', '#b0222a', '#e8582a'],
};

/** Bilberry stems at the foot and the tip: green all year, browner in winter. */
const BILBERRY_STEM: Record<Season, [string, string]> = {
    winter: ['#5a4a32', '#6e7440'],
    spring: ['#4a5a30', '#6a9a3a'],
    summer: ['#4a5a30', '#5e8a36'],
    autumn: ['#5a4a30', '#6e7a38'],
};

/** Stems of a bush, and the stems that carry a berry in summer in place of a leaf. */
const STEMS = 10, BERRIES = 4;

/**
 * Bilberry: a round broom of thin green stems, the inner ones upright and the outer ones leaning,
 * each with a fork. Broad pointed leaves sit on the stem and the fork to alternate sides, folded
 * along the midrib, and one ends each of them; the bush lights as one round tuft. Winter leaves the
 * stems bare, each with three angular twigs; in summer a dark blue berry hangs under the fork of
 * every second stem.
 */
function bilberry(season: Season, rand: () => number): Geometry {
    const leaves = BILBERRY[season];
    const [foot, top] = BILBERRY_STEM[season].map(hex);
    const stem = (p: V) => foot.clone().lerp(top, smooth(0, 0.7, p.z));
    const [berry, bloom] = [hex('#232a5c'), hex('#6672a8')];
    const light = domed(v3(0, 0, 0.15), v3(0.6, 0.6, 0.85), 0.35);
    const cards = sheet();
    const parts: Geometry[] = [];
    // The leaves draw from their own numbers, so the stems stand the same in every season.
    const vary = random(Math.floor(rand() * 1e6));
    /** A stem that tapers from `width` at `from` to a point at `to`. */
    const twig = (from: V, to: V, flat: V, width: number) => cards.add([from.clone().addScaledVector(flat, width / 2), from.clone().addScaledVector(flat, -width / 2), to], [stem(from), stem(from), stem(to)], light);
    /** A leaf from `at` toward `toward`, rising `lift`; each half leans from the midrib. */
    const leaf = (at: V, toward: V, lift: number) => {
        if (leaves === 'bare') return;
        const [deep, mid, tip] = ramp(leaves);
        const length = 0.24 + 0.07 * vary();
        const dir = toward.clone().setZ(0).normalize().setZ(lift).normalize();
        const side = v3().crossVectors(dir, UP).setLength(0.36 * length);
        const body = mid.clone().lerp(tip, 0.7 * vary()).lerp(deep, 0.5 * (1 - smooth(0.1, 0.6, at.z)));
        const [base, end] = [body.clone().lerp(deep, 0.5), body.clone().lerp(tip, 0.4)];
        const [waist, point] = [at.clone().addScaledVector(dir, 0.45 * length), at.clone().addScaledVector(dir, length)];
        for (const half of [-1, 1]) {
            const edge = waist.clone().addScaledVector(side, half).setZ(waist.z + 0.1 * length);
            cards.add([at, edge, point], [base, body, end], (p) => light(p).addScaledVector(side, (-1.2 * half) / length).normalize());
        }
    };
    for (let k = 0; k < STEMS; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const across = v3(-out.y, out.x, 0);
        const tilt = 0.12 + 0.7 * Math.sqrt((k + 0.5) / STEMS);
        const length = (1 - (0.35 * k) / STEMS) * (0.8 + 0.12 * rand());
        const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
        const root = out.clone().multiplyScalar(0.04);
        const end = root.clone().addScaledVector(dir, length);
        const flat = across.clone().applyAxisAngle(dir, (rand() - 0.5) * 1.6);
        const s = k % 2 ? 1 : -1;
        const on = (t: number) => root.clone().addScaledVector(dir, t * length);
        const fork = (t: number, side: number, reach: number) => {
            const tip = on(t).addScaledVector(dir.clone().addScaledVector(across, 0.9 * side).normalize(), reach * length);
            twig(on(t), tip, flat, 0.024);
            return tip;
        };
        twig(root, end, flat, 0.04);
        const tip = fork(0.35, s, 0.5);
        if (leaves === 'bare') {
            fork(0.55, -s, 0.32);
            fork(0.75, s, 0.2);
            continue;
        }
        leaf(end, out, 0.7);
        leaf(tip, tip.clone().sub(root), 0.5);
        leaf(on(0.35).lerp(tip, 0.45), across.clone().multiplyScalar(-s).addScaledVector(out, 0.6), 0.4);
        leaf(on(0.75), across.clone().multiplyScalar(s).addScaledVector(out, 0.5), 0.45);
        if (season === 'summer' && k % 2 && k < 2 * BERRIES) {
            // It hangs under the fork, touching it, a face outward: from beside the bush its outline is six-sided.
            const hang = on(0.35).lerp(tip, 0.8);
            const ball = place(soft(octahedron(0.055)), { at: [hang.x, hang.y, hang.z - 0.05], turn: turnFromTo(v3(1, 1, 1).normalize(), out) });
            parts.push(paint(ball, (p) => berry.clone().lerp(bloom, smooth(hang.z - 0.08, hang.z, p.z))));
        } else leaf(on(0.55), across.clone().multiplyScalar(-s).addScaledVector(out, 0.5), 0.45);
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Bilberry',
    latin: 'Vaccinium myrtillus',
    build: (season, seed) => bilberry(season, random(seed)),
    ground: { on: ['tree', 'shrub', 'grass'], height: [0.2, 0.45], share: 0.25, own: true, clumped: true },
} satisfies Plant;
