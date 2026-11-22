import type { Plant } from '../plant-library';
import { computeNormals, cushion, domed, GOLDEN, hash, hex, leaf, merge, paint, ramp, random, relight, sheet, spike, v3, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Palmito fans: blue grey-green, dustier at the end of the dry summer. */
const PALM: Record<Season, Ramp> = {
    winter: ['#34503f', '#5e8270', '#9cb698'],
    spring: ['#34543f', '#62876e', '#a4c09a'],
    summer: ['#3c503f', '#6a826c', '#a8b692'],
    autumn: ['#365040', '#62826c', '#a0b494'],
};

/** The fans of a clump, the last one dead, and the segments of a fan. */
const FANS = 9, SEGMENTS = 11;

/**
 * A stiff fan facing `facing` from its hinge, continuing `along`: pleated segments over `span` radians, split
 * halfway in, their tips bowed. Two triangles a segment.
 */
function fan(hinge: V, facing: V, along: V, r: number, span: number, rand: () => number): Geometry {
    const side = v3().crossVectors(facing, along).normalize();
    const up = v3().crossVectors(side, facing).normalize();
    const at = (a: number, d: number) => {
        const p = hinge.clone().addScaledVector(up, d * Math.cos(a)).addScaledVector(side, d * Math.sin(a)).addScaledVector(facing, (0.25 * d * d) / r);
        // Segment tips bow a little under their own weight.
        p.z -= (0.12 * d * d) / r;
        return p;
    };
    const points = [hinge];
    for (let i = 0; i <= SEGMENTS; i++) points.push(at(-span / 2 + (i / SEGMENTS) * span, 0.52 * r));
    for (let i = 0; i < SEGMENTS; i++) points.push(at(-span / 2 + ((i + 0.5) / SEGMENTS) * span, r * (0.92 + 0.12 * rand())));
    const index: number[] = [];
    for (let i = 0; i < SEGMENTS; i++) {
        const tip = SEGMENTS + 2 + i;
        index.push(0, 1 + i, tip, 0, tip, 2 + i);
    }
    return computeNormals({ position: points.flatMap((p) => p.toArray()), normal: [], color: [], index });
}

/**
 * Dwarf fan palm: a clump of stiff fans on round stalks out of a short fibrous stem, the inner fans upright and
 * the outer ones held out, round two pale folded spear leaves, and one dead fan hanging straw-brown against the
 * foot. A fan is light along its segments and dark in the folds between, and the fans light together as one round
 * clump. 249 triangles.
 */
function fanPalm(season: Season, seed: number, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(PALM[season]);
    const [straw, strawLit, stalk, fibre] = [hex('#7a6444'), hex('#b09a6e'), hex('#76805a'), hex('#5a4a36')];
    const clump = domed(v3(0, 0, 0.2), v3(1, 1, 1), 0.3);
    const cards = sheet();
    const parts = [cushion([0, 0, 0.085, 0.17], 6, [fibre.clone().multiplyScalar(0.7), fibre, straw], 0.3, seed)];
    for (let k = 0; k < FANS; k++) {
        const a = k * GOLDEN + rand() * 0.5;
        const q = (k + 0.5) / FANS;
        const dead = k === FANS - 1;
        const tilt = dead ? 0.3 : 0.08 + 0.85 * Math.sqrt(q);
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const dir = out.clone().multiplyScalar(Math.sin(tilt)).setZ(Math.cos(tilt));
        const foot = out.clone().multiplyScalar(0.05).setZ(0.08);
        const hinge = foot.clone().addScaledVector(dir, dead ? 0.34 : 0.48 * (1 - 0.3 * q) + 0.08 * rand());
        // Fans face out and up, the outer ones more up; the dead one hangs down the outside of the foot.
        const lift = dead ? 0.46 : 0.3 + 0.45 * q + 0.15 * rand();
        const facing = out.clone().multiplyScalar(Math.cos(lift)).setZ(Math.sin(lift));
        const along = dead ? out.clone().multiplyScalar(0.45).setZ(-0.9) : dir;
        const r = dead ? 0.3 : 0.38 + 0.06 * rand();
        const tone = 0.92 + 0.16 * rand();
        const [fold, body, lit] = dead ? [straw.clone().multiplyScalar(0.75), straw, strawLit] : [deep, mid, tip].map((c) => c.clone().multiplyScalar(tone));
        const blade = relight(fan(hinge, facing, along, r, dead ? 2.4 : 3.3 + 0.4 * rand(), rand), (p) => facing.clone().lerp(clump(p), 0.5).normalize());
        parts.push(paint(blade, (p) => {
            const d = p.distanceTo(hinge) / r;
            return d > 0.7 ? lit.clone().lerp(body, 0.4 * hash(p, k)) : d > 0.3 ? fold.clone().lerp(body, 0.35) : body.clone().lerp(fold, 0.5);
        }));
        spike(cards, foot, hinge.clone().addScaledVector(dir, 0.04), 0.022, dead ? [fibre, straw] : [fibre, stalk], clump);
    }
    // Young leaves stand folded in the middle, so the clump has no hole at its heart.
    for (let k = 0; k < 2; k++) {
        const a = k * Math.PI + rand(), lean = 0.1 + 0.12 * rand();
        leaf(cards, v3(0, 0, 0.15), v3(Math.sin(lean) * Math.cos(a), Math.sin(lean) * Math.sin(a), Math.cos(lean)), 0.5 + 0.12 * rand(), 0.07, 0.3, [mid, mid.clone().lerp(tip, 0.4), tip], clump);
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Dwarf fan palm',
    latin: 'Chamaerops humilis',
    build: (season, seed) => fanPalm(season, seed, random(seed)),
    ground: { on: ['shrub', 'grass', 'bare'], height: [0.6, 1.2], share: 0.06, own: true },
} satisfies Plant;
