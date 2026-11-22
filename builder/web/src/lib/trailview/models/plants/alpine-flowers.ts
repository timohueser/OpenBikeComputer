import type { Plant } from '../plant-library';
import { byHeight, hex, merge, paint, ramp, random, ribbon, sheet, UP, v3, type Colour, type Geometry, type Ramp, type Sheet, type V, type Vec } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Leaves in the grass tuft's summer green; they yellow in autumn and wither in winter, as the meadow does. */
const GREEN: Ramp = ['#557a34', '#86a84a', '#c2c46a'];
const LEAF: Record<Season, Ramp> = { winter: ['#565034', '#766c48', '#968a60'], spring: GREEN, summer: GREEN, autumn: ['#5f6e34', '#8a9448', '#b8ae64'] };

/** A dry seed head: the flower of winter, and of some stalks in autumn. */
const DRY = ['#8a7456', '#9a8462', '#a08a5a'];

/** Gentian, buttercup, edelweiss white, aster and pink, as the season has them. */
const FLOWERS: Record<Season, string[]> = {
    winter: [DRY[0], DRY[1]],
    spring: ['#2f5fd0', '#f2c832', '#f6f2e6', '#2f5fd0'],
    summer: ['#2f5fd0', '#f2c832', '#f6f2e6', '#9a6ad0', '#e48ab0'],
    autumn: ['#9a6ad0', DRY[0], DRY[2]],
};

/**
 * A flower on the end of its stalk at `at`, facing along `axis`: five wedge petals that rise `cup`
 * radii from the middle to their rim, round a raised eye. Eight triangles.
 */
function flower(cards: Sheet, at: V, axis: V, r: number, cup: number, petal: Colour, eye: Colour): void {
    const u = v3().crossVectors(axis, Math.abs(axis.z) < 0.9 ? UP : v3(1, 0, 0)).normalize(), w = v3().crossVectors(axis, u);
    const rim = (a: number, d: number, h: number) => at.clone().addScaledVector(u, d * Math.cos(a)).addScaledVector(w, d * Math.sin(a)).addScaledVector(axis, h);
    const light = () => axis.clone().lerp(UP, 0.5).normalize();
    const throat = petal.clone().lerp(eye, 0.35), turn = Math.atan2(at.y, at.x);
    for (let k = 0; k < 5; k++) {
        const a = turn + (k / 5) * 2 * Math.PI;
        cards.add([at, rim(a - 0.5, r, cup * r), rim(a + 0.5, r, cup * r)], [throat, petal, petal], light);
    }
    const tip = rim(0, 0, (0.3 + 0.35 * cup) * r);
    for (let k = 0; k < 3; k++) cards.add([rim((k / 3) * 2 * Math.PI, 0.3 * r, 0.3 * cup * r), rim(((k + 1) / 3) * 2 * Math.PI, 0.3 * r, 0.3 * cup * r), tip], [eye, eye, eye], light);
}

/**
 * A tuft of meadow flowers: a rosette of fourteen pointed leaves, the outer ones flat and the inner
 * ones raised, and seven flowers on bowed stalks that face up and out. Most flowers of a tuft have
 * one colour, which the seed picks. A dry seed head keeps its petals closed.
 */
function flowers(season: Season, seed: number): Geometry {
    const rand = random(seed);
    const leaf = byHeight(ramp(LEAF[season]), 0.35);
    const stalk = hex(season === 'winter' || season === 'autumn' ? '#8a7a52' : '#5c8a45');
    const colours = FLOWERS[season], main = colours[seed % colours.length];
    const cards = sheet(), parts: Geometry[] = [];
    for (let k = 0; k < 14; k++) {
        const inner = k % 2, a = (k / 14) * 2 * Math.PI + rand() * 0.3, length = (inner ? 0.3 : 0.48) + 0.1 * rand(), rise = inner ? 0.75 : 0.22;
        const out = v3(Math.cos(a), Math.sin(a), 0), side = v3(-out.y, out.x, 0).multiplyScalar(0.2 * length);
        const waist = out.clone().multiplyScalar(0.5 * length).setZ(rise * 0.6 * length), tip = out.clone().multiplyScalar(length).setZ(rise * 0.7 * length);
        const [left, right] = [waist.clone().add(side), waist.clone().sub(side)];
        const light = (p: V) => out.clone().multiplyScalar(p === tip ? 0.5 : 0).add(UP).normalize();
        cards.add([v3(), left, right], [leaf(v3()), leaf(waist), leaf(waist)], light);
        cards.add([left, tip, right], [leaf(waist), leaf(tip), leaf(waist)], light);
    }
    for (let k = 0; k < 7; k++) {
        const a = (k / 7) * 2 * Math.PI + rand() * 0.8;
        const d = 0.12 + 0.3 * rand();
        const h = 0.5 + 0.5 * ((k * 3) % 7) / 6;
        const out = v3(Math.cos(a), Math.sin(a), 0);
        const top: Vec = [d * out.x, d * out.y, h];
        const middle: Vec = [0.3 * top[0], 0.3 * top[1], 0.55 * h];
        parts.push(paint(ribbon([[0, 0, 0], middle, top], [-out.y, out.x, 0], (t) => 0.04 - 0.015 * t, 2), stalk));
        const colour = rand() < 0.6 ? main : colours[Math.floor(rand() * colours.length)], dry = DRY.includes(colour);
        const petal = hex(colour);
        flower(cards, v3(...top), out.multiplyScalar(0.45).add(UP).normalize(), (dry ? 0.09 : 0.15) + 0.03 * rand(), dry ? 1.3 : 0.4, petal, dry ? petal.clone().multiplyScalar(0.6) : hex('#f2d24a'));
    }
    parts.push(cards.build());
    return merge(parts);
}

export default {
    name: 'Alpine flowers',
    latin: 'Gentiana, Ranunculus, Leontopodium, Aster, Dianthus',
    build: flowers,
    ground: { on: ['grass'], height: [0.2, 0.35], share: 0.15, own: true },
} satisfies Plant;
