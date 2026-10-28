import type { Plant } from '../plant-library';
import { blade, GOLDEN, hex, merge, paint, ramp, random, ribbon, smooth, type Geometry, type Ramp, type V } from '../plant-shape';
import type { Season } from '../../rules/regions';

/** Lyme grass blades: blue-grey with a waxy bloom, straw at the tips by autumn. */
const LYME: Record<Season, Ramp> = {
    winter: ['#6a6e64', '#9a9c8c', '#c4bea4'],
    spring: ['#4e6466', '#80989a', '#b4c6c2'],
    summer: ['#4c6264', '#7c9496', '#b0c2be'],
    autumn: ['#5e6a62', '#909c90', '#c4c0a4'],
};

/** The stiff seed heads standing above the leaves: green-grey in summer, straw otherwise. */
const SPIKES: Record<Season, string> = { winter: '#b8ac88', spring: '#8ea098', summer: '#9aa898', autumn: '#c4b48c' };

/**
 * Lyme grass: a stiff tussock of broad blades that stand up and only bow out at the rim, with a few
 * thick seed heads on straight stalks above. 64 triangles.
 */
function lymeGrass(season: Season, rand: () => number): Geometry {
    const [deep, mid, tip] = ramp(LYME[season]);
    const parts: Geometry[] = [];
    for (let k = 0; k < 16; k++) {
        const a = k * GOLDEN + rand() * 0.4;
        const tilt = 0.08 + 0.6 * Math.sqrt(k / 16) * (0.8 + 0.2 * rand());
        const colour = (p: V) => deep.clone().lerp(mid, smooth(0, 0.3, p.length())).lerp(tip, smooth(0.4, 0.95, p.length()));
        const d = 0.03 + 0.08 * Math.sqrt(k / 16);
        parts.push(paint(blade([d * Math.cos(a), d * Math.sin(a), 0], a, 0.75 + 0.2 * rand(), tilt, 0.15 * tilt, (t) => 0.09 * (1 - t), 2), colour));
    }
    const spike = hex(SPIKES[season]);
    for (let k = 0; k < 2; k++) {
        const a = rand() * 2 * Math.PI;
        const top: [number, number, number] = [0.08 * Math.cos(a), 0.08 * Math.sin(a), 0.95 + 0.05 * rand()];
        parts.push(paint(ribbon([[0, 0, 0], [0.5 * top[0], 0.5 * top[1], 0.5 * top[2]], top], [-Math.sin(a), Math.cos(a), 0], (t) => (t < 0.7 ? 0.015 : 0.05), 4), (p) => (p.z > 0.65 ? spike : mid)));
    }
    return merge(parts);
}

export default {
    name: 'Lyme grass',
    latin: 'Leymus arenarius',
    build: (season, seed) => lymeGrass(season, random(seed)),
    ground: { on: ['bare'], height: [0.5, 1], share: 0.05, own: true, clumped: true },
} satisfies Plant;
